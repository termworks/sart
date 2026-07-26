//! Bounded structural validation for the static bootart payload.

use super::{InstallError, MAX_INSTALL_FILE_BYTES};
use std::path::PathBuf;

const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const DT_NULL: i64 = 0;
const DT_NEEDED: i64 = 1;

fn invalid(reason: impl Into<String>) -> InstallError {
    InstallError::InvalidBootartElf(reason.into())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, InstallError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| invalid("truncated u16 field"))?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, InstallError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| invalid("truncated u32 field"))?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, InstallError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| invalid("truncated u64 field"))?;
    Ok(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

fn checked_range(
    offset: u64,
    size: u64,
    length: usize,
) -> Result<std::ops::Range<usize>, InstallError> {
    let end = offset
        .checked_add(size)
        .ok_or_else(|| invalid("ELF table range overflow"))?;
    if end > length as u64 {
        return Err(invalid("ELF table extends beyond payload"));
    }
    let start = usize::try_from(offset).map_err(|_| invalid("ELF offset does not fit usize"))?;
    let end = usize::try_from(end).map_err(|_| invalid("ELF end does not fit usize"))?;
    Ok(start..end)
}

fn expected_machine() -> Result<u16, InstallError> {
    #[cfg(target_arch = "x86_64")]
    {
        Ok(62)
    }
    #[cfg(target_arch = "aarch64")]
    {
        Ok(183)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        Err(invalid(
            "installer supports only x86_64 and aarch64 payloads",
        ))
    }
}

/// Validates one 64-bit little-endian Linux ELF for the build architecture.
/// Dynamic interpreters and DT_NEEDED entries are rejected in-process.
pub fn validate_static_elf(bytes: &[u8]) -> Result<(), InstallError> {
    if bytes.len() as u64 > MAX_INSTALL_FILE_BYTES {
        return Err(InstallError::FileTooLarge {
            path: PathBuf::from(super::BOOTART_BINARY_PATH),
            size: bytes.len() as u64,
            limit: MAX_INSTALL_FILE_BYTES,
        });
    }
    if bytes.len() < ELF_HEADER_SIZE || bytes.get(0..4) != Some(b"\x7fELF") {
        return Err(invalid("missing or truncated ELF header"));
    }
    if bytes[4] != 2 || bytes[5] != 1 || bytes[6] != 1 {
        return Err(invalid(
            "payload must be ELF64, little-endian, current-version",
        ));
    }
    if bytes[7] != 0 && bytes[7] != 3 {
        return Err(invalid("payload has an unsupported ELF OS ABI"));
    }
    let file_type = read_u16(bytes, 16)?;
    if !matches!(file_type, 2 | 3) {
        return Err(invalid("payload must be ET_EXEC or ET_DYN"));
    }
    let machine = read_u16(bytes, 18)?;
    if machine != expected_machine()? {
        return Err(invalid(
            "payload machine does not match the build architecture",
        ));
    }
    if read_u32(bytes, 20)? != 1 || read_u16(bytes, 52)? as usize != ELF_HEADER_SIZE {
        return Err(invalid("invalid ELF version or header size"));
    }
    let entry_point = read_u64(bytes, 24)?;
    let program_offset = read_u64(bytes, 32)?;
    let program_entry_size = read_u16(bytes, 54)? as u64;
    let program_count = read_u16(bytes, 56)? as u64;
    if program_entry_size != PROGRAM_HEADER_SIZE as u64 || program_count == 0 {
        return Err(invalid("invalid or empty program-header table"));
    }
    let table_size = program_entry_size
        .checked_mul(program_count)
        .ok_or_else(|| invalid("program-header table size overflow"))?;
    let table = checked_range(program_offset, table_size, bytes.len())?;

    let mut saw_executable_load = false;
    for index in 0..program_count {
        let header = table.start + (index as usize * PROGRAM_HEADER_SIZE);
        let kind = read_u32(bytes, header)?;
        let flags = read_u32(bytes, header + 4)?;
        let offset = read_u64(bytes, header + 8)?;
        let virtual_address = read_u64(bytes, header + 16)?;
        let file_size = read_u64(bytes, header + 32)?;
        let memory_size = read_u64(bytes, header + 40)?;
        checked_range(offset, file_size, bytes.len())?;
        if memory_size < file_size {
            return Err(invalid("program segment memory size is below file size"));
        }
        match kind {
            PT_LOAD => {
                let memory_end = virtual_address
                    .checked_add(memory_size)
                    .ok_or_else(|| invalid("load segment virtual range overflow"))?;
                if flags & 1 != 0 && entry_point >= virtual_address && entry_point < memory_end {
                    saw_executable_load = true;
                }
            }
            PT_INTERP => return Err(invalid("PT_INTERP is forbidden for the static payload")),
            PT_DYNAMIC => {
                if file_size % 16 != 0 {
                    return Err(invalid("dynamic table has a partial entry"));
                }
                let dynamic = checked_range(offset, file_size, bytes.len())?;
                let mut saw_null = false;
                for entry in bytes[dynamic].chunks_exact(16) {
                    let tag = i64::from_le_bytes(entry[0..8].try_into().expect("eight bytes"));
                    if tag == DT_NEEDED {
                        return Err(invalid("DT_NEEDED is forbidden for the static payload"));
                    }
                    if tag == DT_NULL {
                        saw_null = true;
                        break;
                    }
                }
                if !saw_null {
                    return Err(invalid("dynamic table has no DT_NULL terminator"));
                }
            }
            _ => {}
        }
    }
    if !saw_executable_load {
        return Err(invalid(
            "ELF entry is not inside an executable PT_LOAD segment",
        ));
    }
    Ok(())
}

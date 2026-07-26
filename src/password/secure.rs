use std::error::Error;
use std::fmt;
use std::io;
use std::ptr::NonNull;
use std::sync::atomic::{Ordering, compiler_fence};

/// Hard upper bound for one interactive boot passphrase.
///
/// This is deliberately smaller than the systemd datagram limit and is never
/// configurable from an untrusted request file.
pub const MAX_SECRET_BYTES: usize = 4 * 1024;
pub const DEFAULT_SECRET_BYTES: usize = 1024;

/// Process-wide policy required before a password broker opens any watcher,
/// display, or input resource.
pub trait ProcessSecretPolicy {
    fn protect_process(&self) -> io::Result<()>;
}

/// Linux policy which prevents future core-dump attachment to the broker
/// process via `PR_SET_DUMPABLE=0`.
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxProcessSecretPolicy;

impl ProcessSecretPolicy for LinuxProcessSecretPolicy {
    fn protect_process(&self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: PR_SET_DUMPABLE consumes integer arguments only and does
            // not dereference user pointers.
            if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "process dump protection requires Linux",
            ))
        }
    }
}

/// Best-effort kernel protections applied to the secret mapping.
///
/// A caller may use this to enforce a stricter deployment policy. Failure to
/// lock memory is not itself fatal because small initramfs environments and
/// containers commonly have a zero `RLIMIT_MEMLOCK`; zeroization still applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretProtection {
    locked: bool,
    excluded_from_core: bool,
}

impl SecretProtection {
    pub fn locked(self) -> bool {
        self.locked
    }

    pub fn excluded_from_core(self) -> bool {
        self.excluded_from_core
    }
}

#[derive(Debug)]
pub enum SecretError {
    InvalidCapacity { requested: usize, maximum: usize },
    Allocation(io::Error),
    TooLong { maximum: usize },
    InvalidUtf8,
}

impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCapacity { requested, maximum } => write!(
                formatter,
                "secret capacity {requested} is outside the supported range 1..={maximum}"
            ),
            Self::Allocation(error) => write!(formatter, "allocate protected secret: {error}"),
            Self::TooLong { maximum } => {
                write!(formatter, "secret exceeds the {maximum}-byte limit")
            }
            Self::InvalidUtf8 => formatter.write_str("secret input is not valid UTF-8"),
        }
    }
}

impl Error for SecretError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Allocation(error) => Some(error),
            _ => None,
        }
    }
}

/// A bounded secret held in its own anonymous mapping.
///
/// The type intentionally implements neither `Debug` nor `Clone`. Its complete
/// mapping is overwritten using volatile stores before it is unlocked or
/// unmapped. `mlock(2)` and `MADV_DONTDUMP` are attempted on Linux, but their
/// outcome is reported separately because either facility may be unavailable.
///
/// ```compile_fail
/// use bootart::password::SecureSecret;
/// let secret = SecureSecret::new(32).unwrap();
/// println!("{secret:?}");
/// ```
///
/// ```compile_fail
/// use bootart::password::SecureSecret;
/// let secret = SecureSecret::new(32).unwrap();
/// let duplicate = secret.clone();
/// ```
pub struct SecureSecret {
    pointer: NonNull<u8>,
    length: usize,
    capacity: usize,
    mapping_length: usize,
    protection: SecretProtection,
}

// The allocation is exclusively owned and all mutation requires `&mut self`.
// There is deliberately no Sync implementation.
unsafe impl Send for SecureSecret {}

impl SecureSecret {
    pub fn new(capacity: usize) -> Result<Self, SecretError> {
        if capacity == 0 || capacity > MAX_SECRET_BYTES {
            return Err(SecretError::InvalidCapacity {
                requested: capacity,
                maximum: MAX_SECRET_BYTES,
            });
        }

        let page_size = page_size();
        let mapping_length = capacity
            .checked_add(page_size - 1)
            .map(|value| value / page_size * page_size)
            .ok_or(SecretError::InvalidCapacity {
                requested: capacity,
                maximum: MAX_SECRET_BYTES,
            })?;

        // SAFETY: The requested length is non-zero and page-rounded. The
        // returned mapping is owned exclusively by this value until Drop.
        let raw = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                mapping_length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if raw == libc::MAP_FAILED {
            return Err(SecretError::Allocation(io::Error::last_os_error()));
        }
        let pointer = NonNull::new(raw.cast::<u8>()).expect("mmap never returns a null success");

        // SAFETY: `pointer` names the live mapping described above.
        let locked = unsafe { libc::mlock(pointer.as_ptr().cast(), mapping_length) } == 0;

        #[cfg(target_os = "linux")]
        // SAFETY: `pointer` and `mapping_length` describe the same live mapping.
        let excluded_from_core =
            unsafe { libc::madvise(pointer.as_ptr().cast(), mapping_length, libc::MADV_DONTDUMP) }
                == 0;
        #[cfg(not(target_os = "linux"))]
        let excluded_from_core = false;

        Ok(Self {
            pointer,
            length: 0,
            capacity,
            mapping_length,
            protection: SecretProtection {
                locked,
                excluded_from_core,
            },
        })
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn protection(&self) -> SecretProtection {
        self.protection
    }

    pub fn push_char(&mut self, character: char) -> Result<(), SecretError> {
        let mut encoded = [0_u8; 4];
        self.push_str(character.encode_utf8(&mut encoded))
    }

    pub fn push_str(&mut self, text: &str) -> Result<(), SecretError> {
        let new_length = self
            .length
            .checked_add(text.len())
            .filter(|length| *length <= self.capacity)
            .ok_or(SecretError::TooLong {
                maximum: self.capacity,
            })?;

        // SAFETY: Bounds were checked above; source and destination cannot
        // overlap because `text` cannot borrow mutably from this object.
        unsafe {
            std::ptr::copy_nonoverlapping(
                text.as_ptr(),
                self.pointer.as_ptr().add(self.length),
                text.len(),
            );
        }
        self.length = new_length;
        Ok(())
    }

    /// Remove the final Unicode scalar value, not merely its final byte.
    pub fn pop_char(&mut self) -> Result<Option<char>, SecretError> {
        if self.length == 0 {
            return Ok(None);
        }
        let text = std::str::from_utf8(self.bytes()).map_err(|_| SecretError::InvalidUtf8)?;
        let (start, character) = text
            .char_indices()
            .next_back()
            .ok_or(SecretError::InvalidUtf8)?;
        self.zero_range(start, self.length);
        self.length = start;
        Ok(Some(character))
    }

    /// Expose the live bytes only for the duration of a caller-provided action.
    ///
    /// Callers must not copy or retain the bytes. Bootart uses this solely for
    /// direct vectored writes to private credential transports.
    pub fn expose<R>(&self, action: impl FnOnce(&[u8]) -> R) -> R {
        action(self.bytes())
    }

    /// Overwrite the entire page-rounded mapping, including unused tail bytes.
    pub fn clear(&mut self) {
        self.zero_range(0, self.mapping_length);
        self.length = 0;
    }

    pub(super) fn spare_capacity_mut(&mut self) -> &mut [u8] {
        // SAFETY: The mapping covers at least `capacity` bytes and this method
        // has exclusive access to the object.
        unsafe { std::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.capacity) }
    }

    pub(super) fn commit_received(&mut self, length: usize) -> Result<(), SecretError> {
        if length > self.capacity {
            self.clear();
            return Err(SecretError::TooLong {
                maximum: self.capacity,
            });
        }
        if std::str::from_utf8(&self.spare_capacity_mut()[..length]).is_err() {
            self.clear();
            return Err(SecretError::InvalidUtf8);
        }
        self.length = length;
        self.zero_range(length, self.mapping_length);
        Ok(())
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: `length <= capacity <= mapping_length` is maintained by all
        // constructors and mutators, and the mapping remains live.
        unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.length) }
    }

    fn zero_range(&mut self, start: usize, end: usize) {
        debug_assert!(start <= end);
        debug_assert!(end <= self.mapping_length);
        // SAFETY: The asserted range lies within the live mapping. Volatile
        // writes plus compiler fences prevent dead-store elimination.
        unsafe {
            compiler_fence(Ordering::SeqCst);
            for offset in start..end {
                self.pointer.as_ptr().add(offset).write_volatile(0);
            }
            compiler_fence(Ordering::SeqCst);
        }
    }
}

impl Drop for SecureSecret {
    fn drop(&mut self) {
        self.clear();
        if self.protection.locked {
            // SAFETY: This is the same live mapping previously passed to mlock.
            unsafe {
                libc::munlock(self.pointer.as_ptr().cast(), self.mapping_length);
            }
        }
        // SAFETY: The mapping is still live and is unmapped exactly once here.
        unsafe {
            libc::munmap(self.pointer.as_ptr().cast(), self.mapping_length);
        }
    }
}

fn page_size() -> usize {
    // SAFETY: sysconf has no pointer arguments or memory side effects.
    let value = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(4096)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unbounded_capacity() {
        assert!(matches!(
            SecureSecret::new(0),
            Err(SecretError::InvalidCapacity { .. })
        ));
        assert!(matches!(
            SecureSecret::new(MAX_SECRET_BYTES + 1),
            Err(SecretError::InvalidCapacity { .. })
        ));
    }

    #[test]
    fn removes_complete_utf8_scalar_and_zeroes_tail() {
        let mut secret = SecureSecret::new(32).expect("map secret");
        secret.push_str("ab🔐").expect("append");
        assert_eq!(secret.pop_char().expect("pop"), Some('🔐'));
        assert_eq!(secret.len(), 2);
        assert_eq!(secret.expose(|bytes| bytes.to_vec()), b"ab");
        assert!(
            secret.spare_capacity_mut()[2..]
                .iter()
                .all(|byte| *byte == 0)
        );
    }

    #[test]
    fn clear_overwrites_used_and_unused_bytes() {
        let mut secret = SecureSecret::new(32).expect("map secret");
        secret.push_str("not logged").expect("append");
        secret.clear();
        assert!(secret.is_empty());
        assert!(secret.spare_capacity_mut().iter().all(|byte| *byte == 0));
    }
}

#include "sart/install/installer.hpp"

#include <array>
#include <cstring>
#include <fcntl.h>
#include <limits>
#include <stdexcept>
#include <sys/stat.h>
#include <unistd.h>

namespace sart::install {
    namespace {

        constexpr std::size_t elf_header_size = 64;
        constexpr std::size_t program_header_size = 56;

        template <typename Integer> Integer read_little(std::span<const std::byte> bytes, std::size_t offset) {
            if (offset > bytes.size() || bytes.size() - offset < sizeof(Integer)) {
                throw std::runtime_error("truncated ELF field");
            }
            Integer result = 0;
            for (std::size_t index = 0; index < sizeof(Integer); ++index) {
                result |= static_cast<Integer>(std::to_integer<unsigned char>(bytes[offset + index])) << (index * 8);
            }
            return result;
        }

        void checked_range(std::uint64_t offset, std::uint64_t size, std::size_t length) {
            if (offset > length || size > length - offset) {
                throw std::runtime_error("ELF table extends beyond payload");
            }
        }

        std::uint16_t expected_machine() {
#if defined(__x86_64__)
            return 62;
#elif defined(__aarch64__)
            return 183;
#else
#error "Sart installer supports x86_64 and aarch64"
#endif
        }

    } // namespace

    void validate_static_elf(std::span<const std::byte> bytes) {
        if (bytes.size() > max_install_file_bytes) {
            throw std::runtime_error("Sart ELF exceeds the installer size limit");
        }
        constexpr std::array magic{std::byte{0x7f}, std::byte{'E'}, std::byte{'L'}, std::byte{'F'}};
        if (bytes.size() < elf_header_size || !std::equal(magic.begin(), magic.end(), bytes.begin())) {
            throw std::runtime_error("missing or truncated ELF header");
        }
        if (bytes[4] != std::byte{2} || bytes[5] != std::byte{1} || bytes[6] != std::byte{1}) {
            throw std::runtime_error("payload must be ELF64 little-endian current-version");
        }
        if (bytes[7] != std::byte{0} && bytes[7] != std::byte{3}) {
            throw std::runtime_error("payload has an unsupported ELF OS ABI");
        }
        const auto type = read_little<std::uint16_t>(bytes, 16);
        if (type != 2 && type != 3) {
            throw std::runtime_error("payload must be ET_EXEC or ET_DYN");
        }
        if (read_little<std::uint16_t>(bytes, 18) != expected_machine()) {
            throw std::runtime_error("payload machine does not match Sart");
        }
        if (read_little<std::uint32_t>(bytes, 20) != 1 || read_little<std::uint16_t>(bytes, 52) != elf_header_size) {
            throw std::runtime_error("invalid ELF version or header size");
        }
        const auto entry = read_little<std::uint64_t>(bytes, 24);
        const auto table_offset = read_little<std::uint64_t>(bytes, 32);
        const auto entry_size = read_little<std::uint16_t>(bytes, 54);
        const auto count = read_little<std::uint16_t>(bytes, 56);
        if (entry_size != program_header_size || count == 0) {
            throw std::runtime_error("invalid or empty program-header table");
        }
        const auto table_size = static_cast<std::uint64_t>(entry_size) * count;
        checked_range(table_offset, table_size, bytes.size());
        bool executable_entry = false;
        for (std::uint16_t index = 0; index < count; ++index) {
            const auto header = static_cast<std::size_t>(table_offset) + index * program_header_size;
            const auto kind = read_little<std::uint32_t>(bytes, header);
            const auto flags = read_little<std::uint32_t>(bytes, header + 4);
            const auto offset = read_little<std::uint64_t>(bytes, header + 8);
            const auto address = read_little<std::uint64_t>(bytes, header + 16);
            const auto file_size = read_little<std::uint64_t>(bytes, header + 32);
            const auto memory_size = read_little<std::uint64_t>(bytes, header + 40);
            checked_range(offset, file_size, bytes.size());
            if (memory_size < file_size) {
                throw std::runtime_error("program segment memory size is below file size");
            }
            if (kind == 1) {
                if (address > std::numeric_limits<std::uint64_t>::max() - memory_size) {
                    throw std::runtime_error("load segment virtual range overflow");
                }
                executable_entry |= (flags & 1U) != 0 && entry >= address && entry < address + memory_size;
            } else if (kind == 3) {
                throw std::runtime_error("PT_INTERP is forbidden for the static payload");
            } else if (kind == 2) {
                if (file_size % 16 != 0) {
                    throw std::runtime_error("dynamic table has a partial entry");
                }
                bool terminated = false;
                for (std::uint64_t at = offset; at < offset + file_size; at += 16) {
                    const auto tag = read_little<std::uint64_t>(bytes, static_cast<std::size_t>(at));
                    if (tag == 1) {
                        throw std::runtime_error("DT_NEEDED is forbidden for the static payload");
                    }
                    if (tag == 0) {
                        terminated = true;
                        break;
                    }
                }
                if (!terminated) {
                    throw std::runtime_error("dynamic table has no DT_NULL terminator");
                }
            }
        }
        if (!executable_entry) {
            throw std::runtime_error("ELF entry is not inside an executable PT_LOAD segment");
        }
    }

    std::vector<std::byte> read_running_elf() {
        const int descriptor = open("/proc/self/exe", O_RDONLY | O_CLOEXEC);
        if (descriptor < 0) {
            throw std::runtime_error(std::string("cannot open /proc/self/exe: ") + std::strerror(errno));
        }
        struct Guard {
            int descriptor;
            ~Guard() { close(descriptor); }
        } guard{descriptor};
        struct stat status{};
        if (fstat(descriptor, &status) != 0 || !S_ISREG(status.st_mode) || status.st_size < 0) {
            throw std::runtime_error("running Sart descriptor is not a regular file");
        }
        if (static_cast<std::uint64_t>(status.st_size) > max_install_file_bytes) {
            throw std::runtime_error("running Sart ELF exceeds the installer size limit");
        }
        std::vector<std::byte> bytes(static_cast<std::size_t>(status.st_size));
        std::size_t offset = 0;
        while (offset < bytes.size()) {
            const auto count = read(descriptor, bytes.data() + offset, bytes.size() - offset);
            if (count < 0 && errno == EINTR) {
                continue;
            }
            if (count <= 0) {
                throw std::runtime_error("running Sart ELF changed while being read");
            }
            offset += static_cast<std::size_t>(count);
        }
        std::byte extra{};
        if (read(descriptor, &extra, 1) != 0) {
            throw std::runtime_error("running Sart ELF changed size while being read");
        }
        validate_static_elf(bytes);
        return bytes;
    }

} // namespace sart::install

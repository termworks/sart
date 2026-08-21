#include "sart/install/backends.hpp"

#include "sart/core/sha256.hpp"

#include <algorithm>
#include <array>
#include <bit>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <filesystem>
#include <format>
#include <linux/fs.h>
#include <stdexcept>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <unistd.h>
#include <utility>

namespace sart::install {
    namespace {

        class Descriptor {
          public:
            explicit Descriptor(int value) : value_(value) {}
            Descriptor(const Descriptor &) = delete;
            Descriptor &operator=(const Descriptor &) = delete;
            Descriptor(Descriptor &&other) noexcept : value_(std::exchange(other.value_, -1)) {}
            ~Descriptor() {
                if (value_ >= 0)
                    close(value_);
            }
            int get() const noexcept { return value_; }

          private:
            int value_;
        };

        std::runtime_error raw_error(std::string_view action, const std::filesystem::path &path) {
            return std::runtime_error(std::format("{} {}: {}", action, path.string(), std::strerror(errno)));
        }

        bool safe_label(std::string_view label) {
            return !label.empty() && label.size() <= 64 && std::ranges::all_of(label, [](unsigned char byte) {
                return (byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') || (byte >= 'a' && byte <= 'z') ||
                       byte == '_' || byte == '-' || byte == '.';
            });
        }

        std::filesystem::path resolve_label(std::string_view label) {
            if (!safe_label(label))
                throw std::runtime_error("Android boot partition label is unsafe");
            const auto link = std::filesystem::path("/dev/disk/by-partlabel") / std::string(label);
            struct stat status{};
            if (lstat(link.c_str(), &status) != 0)
                throw raw_error("inspect Android boot partition label", link);
            if (!S_ISLNK(status.st_mode) || status.st_uid != 0)
                throw std::runtime_error("Android boot partition label is not a root-owned symlink");
            std::error_code error;
            const auto canonical = std::filesystem::canonical(link, error);
            if (error)
                throw std::runtime_error("resolve Android boot partition label: " + error.message());
            const auto text = canonical.string();
            if (!text.starts_with("/dev/") || text.contains("//"))
                throw std::runtime_error("Android boot partition resolves outside /dev");
            for (const auto &component : canonical) {
                if (component == "." || component == "..")
                    throw std::runtime_error("Android boot partition has an unsafe canonical path");
            }
            return canonical;
        }

        std::uint64_t partition_size(int descriptor, const std::filesystem::path &path) {
            std::uint64_t bytes{};
            static_assert(sizeof(int) == sizeof(std::uint32_t));
            constexpr auto request = std::bit_cast<int>(std::uint32_t{BLKGETSIZE64});
            if (ioctl(descriptor, request, &bytes) != 0)
                throw raw_error("read Android boot partition size", path);
            if (bytes == 0 || bytes > max_transaction_bytes)
                throw std::runtime_error("Android boot partition is outside the transaction size bound");
            return bytes;
        }

        Descriptor open_partition(const AndroidBootPartitionFact &expected, bool writable) {
            if (!safe_android_partition_fact(expected))
                throw std::runtime_error("Android boot partition identity is unsafe");
            const auto canonical = resolve_label(expected.label);
            if (canonical != std::filesystem::path(expected.canonical_path))
                throw std::runtime_error("Android boot partition label changed its target");
            const int flags = (writable ? O_RDWR : O_RDONLY) | O_NOFOLLOW | O_CLOEXEC;
            const int raw = open(canonical.c_str(), flags);
            if (raw < 0)
                throw raw_error("open Android boot partition", canonical);
            Descriptor descriptor(raw);
            struct stat status{};
            if (fstat(raw, &status) != 0)
                throw raw_error("inspect Android boot partition", canonical);
            if (!S_ISBLK(status.st_mode) || status.st_uid != 0 || (status.st_mode & 0002) != 0 ||
                static_cast<std::uint64_t>(status.st_rdev) != expected.device_number ||
                resolve_label(expected.label) != canonical) {
                throw std::runtime_error("Android boot partition changed label, type, owner, mode, or device number");
            }
            if (partition_size(raw, canonical) != expected.bytes)
                throw std::runtime_error("Android boot partition changed size");
            return descriptor;
        }

        std::vector<std::byte> pread_partition(int descriptor, std::size_t bytes, const std::filesystem::path &path) {
            std::vector<std::byte> output(bytes);
            std::size_t offset = 0;
            while (offset < output.size()) {
                const auto count =
                    pread(descriptor, output.data() + offset, output.size() - offset, static_cast<off_t>(offset));
                if (count < 0 && errno == EINTR)
                    continue;
                if (count <= 0)
                    throw raw_error("read complete Android boot partition", path);
                offset += static_cast<std::size_t>(count);
            }
            return output;
        }

        void durable_write(int descriptor, std::span<const std::byte> bytes, const std::filesystem::path &path,
                           std::string_view operation) {
            constexpr std::size_t chunk_bytes = 1024 * 1024;
            std::size_t offset = 0;
            while (offset < bytes.size()) {
                const auto amount = std::min(chunk_bytes, bytes.size() - offset);
                std::size_t written = 0;
                while (written < amount) {
                    const auto count = pwrite(descriptor, bytes.data() + offset + written, amount - written,
                                              static_cast<off_t>(offset + written));
                    if (count < 0 && errno == EINTR)
                        continue;
                    if (count <= 0)
                        throw raw_error(std::string(operation) + " Android boot partition", path);
                    written += static_cast<std::size_t>(count);
                }
                if (fdatasync(descriptor) != 0)
                    throw raw_error(std::string("synchronize Android boot ") + std::string(operation) + " chunk", path);
                offset += amount;
            }
            if (fsync(descriptor) != 0)
                throw raw_error(std::string("synchronize Android boot ") + std::string(operation), path);
        }

        void restore_descriptor(int descriptor, const AndroidBootPartitionFact &partition,
                                std::span<const std::byte> original) {
            durable_write(descriptor, original, partition.canonical_path, "restoration");
            const auto restored = pread_partition(descriptor, original.size(), partition.canonical_path);
            if (!std::ranges::equal(restored, original))
                throw std::runtime_error("Android boot partition restoration failed read-back verification");
        }

    } // namespace

    bool safe_android_partition_fact(const AndroidBootPartitionFact &partition) {
        if (partition.device_number == 0 || partition.bytes == 0 || partition.bytes > max_transaction_bytes ||
            !safe_label(partition.label) || !partition.canonical_path.starts_with("/dev/") ||
            partition.canonical_path.contains("//") || partition.digest.size() != 64) {
            return false;
        }
        const std::filesystem::path path(partition.canonical_path);
        if (!path.is_absolute())
            return false;
        return std::ranges::all_of(
            path, [](const std::filesystem::path &component) { return component != "." && component != ".."; });
    }

    AndroidBootPartitionFact inspect_android_boot_partition(std::string_view label) {
        const auto canonical = resolve_label(label);
        const int raw = open(canonical.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
        if (raw < 0)
            throw raw_error("open Android boot partition", canonical);
        Descriptor descriptor(raw);
        struct stat status{};
        if (fstat(raw, &status) != 0)
            throw raw_error("inspect Android boot partition", canonical);
        if (!S_ISBLK(status.st_mode) || status.st_uid != 0 || (status.st_mode & 0002) != 0 ||
            resolve_label(label) != canonical) {
            throw std::runtime_error("Android boot partition changed label, type, owner, or mode");
        }
        const auto bytes = partition_size(raw, canonical);
        const auto contents = pread_partition(raw, static_cast<std::size_t>(bytes), canonical);
        AndroidBootPartitionFact result{std::string(label), canonical.string(),
                                        static_cast<std::uint64_t>(status.st_rdev), bytes, sha256(contents)};
        if (!safe_android_partition_fact(result))
            throw std::runtime_error("Android boot partition identity is outside the bounded contract");
        return result;
    }

    std::vector<std::byte> read_android_boot_partition(const AndroidBootPartitionFact &partition,
                                                       bool require_discovery_digest) {
        auto descriptor = open_partition(partition, false);
        auto contents =
            pread_partition(descriptor.get(), static_cast<std::size_t>(partition.bytes), partition.canonical_path);
        if (require_discovery_digest && sha256(contents) != partition.digest)
            throw std::runtime_error("Android boot partition changed after discovery");
        return contents;
    }

    std::string activate_android_boot_partition_durable(const AndroidBootPartitionFact &partition,
                                                        std::span<const std::byte> original,
                                                        std::span<const std::byte> candidate) {
        if (original.size() != partition.bytes || original.empty() || candidate.empty() ||
            candidate.size() > original.size()) {
            throw std::runtime_error("Android boot activation exceeds the bounded partition contract");
        }
        auto descriptor = open_partition(partition, true);
        const auto current = pread_partition(descriptor.get(), original.size(), partition.canonical_path);
        if (!std::ranges::equal(current, original))
            throw std::runtime_error("Android boot partition changed after its preimage was captured");
        try {
            durable_write(descriptor.get(), candidate, partition.canonical_path, "activation");
            auto expected = std::vector<std::byte>(original.begin(), original.end());
            std::ranges::copy(candidate, expected.begin());
            const auto installed = pread_partition(descriptor.get(), original.size(), partition.canonical_path);
            if (installed != expected)
                throw std::runtime_error("Android boot partition activation failed read-back verification");
            return sha256(installed);
        } catch (...) {
            const auto failure = std::current_exception();
            restore_descriptor(descriptor.get(), partition, original);
            std::rethrow_exception(failure);
        }
    }

    void restore_android_boot_partition_durable(const AndroidBootPartitionFact &partition,
                                                std::span<const std::byte> original) {
        if (original.size() != partition.bytes || original.empty())
            throw std::runtime_error("Android boot restore exceeds the bounded partition contract");
        auto descriptor = open_partition(partition, true);
        restore_descriptor(descriptor.get(), partition, original);
    }

} // namespace sart::install

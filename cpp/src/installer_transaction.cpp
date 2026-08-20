#include "bootart/installer.hpp"

#include "bootart/installer_live.hpp"
#include "bootart/integration_patch.hpp"
#include "bootart/integration_resources.hpp"
#include "bootart/sha256.hpp"

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <charconv>
#include <chrono>
#include <cstring>
#include <fcntl.h>
#include <filesystem>
#include <format>
#include <limits>
#include <map>
#include <optional>
#include <set>
#include <sstream>
#include <stdexcept>
#include <sys/file.h>
#include <sys/stat.h>
#include <tuple>
#include <type_traits>
#include <unistd.h>
#include <utility>

namespace bootart::install {
    namespace {

        constexpr std::string_view manifest_path = "/var/lib/bootart/install/manifest.v1";
        constexpr std::string_view journal_path = "/.bootart-installer-journal.v1";
        constexpr std::string_view transactions_path = "/var/lib/bootart/install/transactions";
        constexpr std::size_t max_state_bytes = 1024 * 1024;
        constexpr std::size_t max_entries = 4096;
        std::atomic<std::uint64_t> unique_counter{};

        enum class PreimageKind : std::uint8_t { absent, file, symlink };
        enum class EntryKind : std::uint8_t { file, patched_file, symlink, transient_file };
        enum class Progress : std::uint8_t { planned, in_progress, applied };
        enum class JournalKind : std::uint8_t { install, refresh, uninstall };

        struct Preimage {
            PreimageKind kind{PreimageKind::absent};
            std::uint16_t mode{};
            std::string digest;
            std::string target;
            std::string backup;
            std::vector<std::byte> captured;
        };

        struct Entry {
            EntryKind kind{EntryKind::file};
            std::string path;
            std::uint16_t installed_mode{};
            std::string installed_digest;
            std::string installed_target;
            Preimage original;
            Progress progress{Progress::planned};
            std::vector<std::byte> content;
        };

        struct RawBootManifest {
            AndroidBootPartitionFact partition;
            std::string original_digest;
            std::string installed_digest;
            std::string backup;
        };

        struct RawBootJournal {
            AndroidBootPartitionFact partition;
            std::string original_digest;
            std::string installed_digest;
            std::string backup;
            Progress progress{Progress::planned};
        };

        struct Manifest {
            std::string transaction;
            std::string plan_id;
            AdapterId initramfs;
            AdapterId real_root;
            std::optional<DracutSystemdImageRecord> image;
            std::optional<RawBootManifest> raw_boot;
            std::vector<Entry> entries;
            std::vector<std::string> created_dirs;
        };

        struct Journal {
            JournalKind kind;
            std::string phase;
            std::string transaction;
            std::vector<Entry> entries;
            std::vector<std::string> created_dirs;
            std::optional<RawBootJournal> raw_boot;
        };

        struct RootLock {
            int descriptor{-1};
            RootLock() = default;
            RootLock(const RootLock &) = delete;
            RootLock &operator=(const RootLock &) = delete;
            RootLock(RootLock &&other) noexcept : descriptor(std::exchange(other.descriptor, -1)) {}
            ~RootLock() {
                if (descriptor >= 0)
                    close(descriptor);
            }
        };

        std::runtime_error system_error(std::string_view action, const std::filesystem::path &path) {
            return std::runtime_error(std::format("{} {}: {}", action, path.string(), std::strerror(errno)));
        }

        std::filesystem::path host_path(std::string_view root, std::string_view guest) {
            if (guest.empty() || guest.front() != '/' || guest.size() > 4096 || guest.contains('\0') ||
                guest.contains("//") || guest.contains("/../") || guest.ends_with("/..") || guest.contains("/./") ||
                guest.ends_with("/.")) {
                throw std::runtime_error("unsafe installer path");
            }
            if (root == "/") {
                return std::filesystem::path(guest);
            }
            return std::filesystem::path(root) / guest.substr(1);
        }

        struct stat lstat_optional(const std::filesystem::path &path, bool &exists) {
            struct stat status{};
            if (lstat(path.c_str(), &status) == 0) {
                exists = true;
                return status;
            }
            if (errno == ENOENT) {
                exists = false;
                return {};
            }
            throw system_error("inspect", path);
        }

        void validate_directory(const std::filesystem::path &path, std::uint32_t owner) {
            bool exists = false;
            const auto status = lstat_optional(path, exists);
            if (!exists || !S_ISDIR(status.st_mode) || S_ISLNK(status.st_mode) || status.st_uid != owner ||
                (status.st_mode & 0022) != 0) {
                throw std::runtime_error("installer directory changed type, owner, or mode: " + path.string());
            }
        }

        void validate_root(std::string_view root, std::uint32_t owner) {
            if (root.empty() || root.front() != '/' || root.contains("/../") || root.ends_with("/..")) {
                throw std::runtime_error("installer root is not a safe absolute path");
            }
            validate_directory(std::filesystem::path(root), owner);
        }

        RootLock lock_root(std::string_view root, std::uint32_t owner) {
            validate_root(root, owner);
            const int descriptor = open(std::string(root).c_str(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
            if (descriptor < 0)
                throw system_error("open installer root", std::filesystem::path(root));
            RootLock lock;
            lock.descriptor = descriptor;
            struct stat status{};
            if (fstat(descriptor, &status) != 0 || !S_ISDIR(status.st_mode) || status.st_uid != owner ||
                (status.st_mode & 0022) != 0) {
                throw std::runtime_error("opened installer root changed type, owner, or mode");
            }
            if (flock(descriptor, LOCK_EX | LOCK_NB) != 0) {
                if (errno == EWOULDBLOCK || errno == EAGAIN) {
                    throw std::runtime_error("another installer transaction holds the root lock");
                }
                throw system_error("lock installer root", std::filesystem::path(root));
            }
            return lock;
        }

        void fsync_directory(const std::filesystem::path &path) {
            const int descriptor = open(path.c_str(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
            if (descriptor < 0)
                throw system_error("open directory for sync", path);
            if (fsync(descriptor) != 0) {
                const auto error = system_error("sync directory", path);
                close(descriptor);
                throw error;
            }
            close(descriptor);
        }

        void write_all(int descriptor, std::span<const std::byte> bytes, const std::filesystem::path &path) {
            while (!bytes.empty()) {
                const auto count = write(descriptor, bytes.data(), bytes.size());
                if (count < 0 && errno == EINTR)
                    continue;
                if (count <= 0)
                    throw system_error("write", path);
                bytes = bytes.subspan(static_cast<std::size_t>(count));
            }
        }

        std::string temporary_name() {
            return std::format(".bootart-tmp-{}-{}", getpid(), unique_counter.fetch_add(1));
        }

        void atomic_write(const std::filesystem::path &path, std::span<const std::byte> bytes, std::uint16_t mode) {
            const auto parent = path.parent_path();
            const auto temporary = parent / temporary_name();
            const int descriptor = open(temporary.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
            if (descriptor < 0)
                throw system_error("create temporary", temporary);
            bool open_file = true;
            try {
                write_all(descriptor, bytes, temporary);
                if (fchmod(descriptor, mode) != 0)
                    throw system_error("set mode", temporary);
                if (fsync(descriptor) != 0)
                    throw system_error("sync", temporary);
                if (close(descriptor) != 0)
                    throw system_error("close", temporary);
                open_file = false;
                if (rename(temporary.c_str(), path.c_str()) != 0)
                    throw system_error("replace", path);
                fsync_directory(parent);
            } catch (...) {
                if (open_file)
                    close(descriptor);
                unlink(temporary.c_str());
                throw;
            }
        }

        void atomic_write_text(const std::filesystem::path &path, std::string_view text, std::uint16_t mode) {
            atomic_write(path, std::as_bytes(std::span(text.data(), text.size())), mode);
        }

        void atomic_symlink(const std::filesystem::path &path, std::string_view target) {
            const auto temporary = path.parent_path() / temporary_name();
            if (symlink(std::string(target).c_str(), temporary.c_str()) != 0) {
                throw system_error("create symlink", temporary);
            }
            if (rename(temporary.c_str(), path.c_str()) != 0) {
                const auto error = system_error("replace symlink", path);
                unlink(temporary.c_str());
                throw error;
            }
            fsync_directory(path.parent_path());
        }

        std::vector<std::byte> read_regular(const std::filesystem::path &path, std::uint32_t owner,
                                            std::uint16_t *mode = nullptr, std::size_t limit = max_transaction_bytes) {
            const int descriptor = open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
            if (descriptor < 0)
                throw system_error("open", path);
            struct stat status{};
            if (fstat(descriptor, &status) != 0 || !S_ISREG(status.st_mode) || status.st_nlink != 1 ||
                status.st_uid != owner || (status.st_mode & 0022) != 0 || status.st_size < 0 ||
                static_cast<std::uint64_t>(status.st_size) > limit) {
                close(descriptor);
                throw std::runtime_error("installer file changed type, owner, mode, link count, or size: " +
                                         path.string());
            }
            std::vector<std::byte> bytes(static_cast<std::size_t>(status.st_size));
            std::size_t offset = 0;
            while (offset < bytes.size()) {
                const auto count = read(descriptor, bytes.data() + offset, bytes.size() - offset);
                if (count < 0 && errno == EINTR)
                    continue;
                if (count <= 0) {
                    close(descriptor);
                    throw std::runtime_error("installer file changed while being read: " + path.string());
                }
                offset += static_cast<std::size_t>(count);
            }
            std::byte extra{};
            if (read(descriptor, &extra, 1) != 0) {
                close(descriptor);
                throw std::runtime_error("installer file changed size while being read: " + path.string());
            }
            close(descriptor);
            if (mode != nullptr)
                *mode = status.st_mode & 07777;
            return bytes;
        }

        std::optional<std::vector<std::byte>> read_optional_document(std::string_view root, std::string_view guest,
                                                                     std::uint32_t owner) {
            const auto path = host_path(root, guest);
            bool exists = false;
            static_cast<void>(lstat_optional(path, exists));
            if (!exists)
                return std::nullopt;
            return read_regular(path, owner, nullptr, max_state_bytes);
        }

        std::string hex(std::string_view input) {
            constexpr std::string_view digits = "0123456789abcdef";
            std::string output;
            output.reserve(input.size() * 2);
            for (const unsigned char byte : input) {
                output.push_back(digits[byte >> 4]);
                output.push_back(digits[byte & 15]);
            }
            return output;
        }

        unsigned hex_digit(char value) {
            if (value >= '0' && value <= '9')
                return value - '0';
            if (value >= 'a' && value <= 'f')
                return value - 'a' + 10;
            throw std::runtime_error("state document has invalid hexadecimal text");
        }

        std::string unhex(std::string_view input) {
            if (input.size() % 2 != 0)
                throw std::runtime_error("state document has odd hexadecimal text");
            std::string output;
            output.reserve(input.size() / 2);
            for (std::size_t index = 0; index < input.size(); index += 2) {
                output.push_back(static_cast<char>((hex_digit(input[index]) << 4) | hex_digit(input[index + 1])));
            }
            return output;
        }

        std::vector<std::string_view> fields(std::string_view line) {
            std::vector<std::string_view> output;
            while (true) {
                const auto at = line.find('\t');
                output.push_back(line.substr(0, at));
                if (at == std::string_view::npos)
                    break;
                line.remove_prefix(at + 1);
            }
            return output;
        }

        std::uint64_t number(std::string_view value) {
            if (value.empty() || (value.size() > 1 && value.front() == '0'))
                throw std::runtime_error("state document contains a noncanonical integer");
            std::uint64_t result{};
            const auto [end, error] = std::from_chars(value.data(), value.data() + value.size(), result);
            if (error != std::errc{} || end != value.data() + value.size()) {
                throw std::runtime_error("state document contains an invalid integer");
            }
            return result;
        }

        bool digest_text(std::string_view value) {
            return value.size() == 64 && std::ranges::all_of(value, [](unsigned char byte) {
                       return (byte >= '0' && byte <= '9') || (byte >= 'a' && byte <= 'f');
                   });
        }

        bool transaction_text(std::string_view value) {
            return !value.empty() && value.size() <= 128 && std::ranges::all_of(value, [](unsigned char byte) {
                return (byte >= '0' && byte <= '9') || (byte >= 'A' && byte <= 'Z') || (byte >= 'a' && byte <= 'z') ||
                       byte == '-';
            });
        }

        std::uint16_t mode_number(std::string_view value) {
            const auto parsed = number(value);
            if (parsed > 07777)
                throw std::runtime_error("state document contains an invalid mode");
            return static_cast<std::uint16_t>(parsed);
        }

        std::string_view preimage_name(PreimageKind kind) {
            switch (kind) {
            case PreimageKind::absent:
                return "absent";
            case PreimageKind::file:
                return "file";
            case PreimageKind::symlink:
                return "symlink";
            }
            throw std::runtime_error("invalid preimage kind");
        }

        PreimageKind parse_preimage_kind(std::string_view value) {
            if (value == "absent")
                return PreimageKind::absent;
            if (value == "file")
                return PreimageKind::file;
            if (value == "symlink")
                return PreimageKind::symlink;
            throw std::runtime_error("state document contains an invalid preimage kind");
        }

        std::string_view entry_name(EntryKind kind) {
            switch (kind) {
            case EntryKind::file:
                return "file";
            case EntryKind::patched_file:
                return "patch";
            case EntryKind::symlink:
                return "symlink";
            case EntryKind::transient_file:
                return "transient";
            }
            throw std::runtime_error("invalid entry kind");
        }

        EntryKind parse_entry_kind(std::string_view value) {
            if (value == "file")
                return EntryKind::file;
            if (value == "patch")
                return EntryKind::patched_file;
            if (value == "symlink")
                return EntryKind::symlink;
            if (value == "transient")
                return EntryKind::transient_file;
            throw std::runtime_error("state document contains an invalid entry kind");
        }

        std::string_view progress_name(Progress progress) {
            switch (progress) {
            case Progress::planned:
                return "planned";
            case Progress::in_progress:
                return "in-progress";
            case Progress::applied:
                return "applied";
            }
            throw std::runtime_error("invalid entry progress");
        }

        Progress parse_progress(std::string_view value) {
            if (value == "planned")
                return Progress::planned;
            if (value == "in-progress")
                return Progress::in_progress;
            if (value == "applied")
                return Progress::applied;
            throw std::runtime_error("state document contains invalid progress");
        }

        AdapterId parse_adapter(std::string_view value) {
            for (const auto id : adapter_ids) {
                if (adapter_name(id) == value)
                    return id;
            }
            throw std::runtime_error("state document contains an invalid adapter");
        }

        std::string serialize_entry(const Entry &entry, bool journal) {
            return std::format("entry\t{}{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                               journal ? std::string(progress_name(entry.progress)) + "\t" : "", entry_name(entry.kind),
                               hex(entry.path), entry.installed_mode, entry.installed_digest,
                               hex(entry.installed_target), preimage_name(entry.original.kind), entry.original.mode,
                               entry.original.digest, hex(entry.original.target), hex(entry.original.backup));
        }

        Entry parse_entry(const std::vector<std::string_view> &item, bool journal) {
            const std::size_t expected = journal ? 12 : 11;
            if (item.size() != expected || item[0] != "entry") {
                throw std::runtime_error("state document contains a malformed entry");
            }
            std::size_t at = 1;
            Entry entry{};
            if (journal)
                entry.progress = parse_progress(item[at++]);
            entry.kind = parse_entry_kind(item[at++]);
            entry.path = unhex(item[at++]);
            entry.installed_mode = mode_number(item[at++]);
            entry.installed_digest = std::string(item[at++]);
            entry.installed_target = unhex(item[at++]);
            entry.original.kind = parse_preimage_kind(item[at++]);
            entry.original.mode = mode_number(item[at++]);
            entry.original.digest = std::string(item[at++]);
            entry.original.target = unhex(item[at++]);
            entry.original.backup = unhex(item[at++]);
            host_path("/", entry.path);
            if (!entry.original.backup.empty())
                host_path("/", entry.original.backup);
            if (entry.kind == EntryKind::symlink) {
                if (entry.installed_mode != 0 || !entry.installed_digest.empty() || entry.installed_target.empty() ||
                    entry.installed_target.contains('\0')) {
                    throw std::runtime_error("state document contains an invalid symlink entry");
                }
            } else if (!entry.installed_target.empty() ||
                       (!entry.installed_digest.empty() && !digest_text(entry.installed_digest))) {
                throw std::runtime_error("state document contains an invalid file entry");
            }
            if (entry.original.kind == PreimageKind::absent) {
                if (entry.original.mode != 0 || !entry.original.digest.empty() || !entry.original.target.empty() ||
                    !entry.original.backup.empty()) {
                    throw std::runtime_error("state document contains an invalid absent preimage");
                }
            } else if (entry.original.kind == PreimageKind::file) {
                if (!digest_text(entry.original.digest) || !entry.original.target.empty()) {
                    throw std::runtime_error("state document contains an invalid file preimage");
                }
            } else if (entry.original.mode != 0 || !entry.original.digest.empty() || entry.original.target.empty() ||
                       entry.original.target.contains('\0') || !entry.original.backup.empty()) {
                throw std::runtime_error("state document contains an invalid symlink preimage");
            }
            return entry;
        }

        std::string_view image_kind(AdapterId initramfs, AdapterId real_root) {
            if (initramfs == AdapterId::dracut_systemd && real_root == AdapterId::systemd_real_root)
                return "dracut-systemd";
            if (initramfs == AdapterId::initramfs_tools_busybox && real_root == AdapterId::systemd_real_root)
                return "initramfs-tools-systemd";
            if (initramfs == AdapterId::mkinitcpio_busybox && real_root == AdapterId::systemd_real_root)
                return "mkinitcpio-systemd";
            if (initramfs == AdapterId::mkinitfs_busybox && real_root == AdapterId::openrc_real_root)
                return "mkinitfs-openrc";
            if (initramfs == AdapterId::mkinitfs_boot_deploy && real_root == AdapterId::openrc_real_root)
                return "mkinitfs-boot-deploy-openrc";
            if (initramfs == AdapterId::mkinitfs_boot_deploy && real_root == AdapterId::systemd_real_root)
                return "mkinitfs-boot-deploy-systemd";
            throw std::runtime_error("installer image has an unsupported adapter pair");
        }

        void validate_image_record(const DracutSystemdImageRecord &record, AdapterId initramfs, AdapterId real_root) {
            static_cast<void>(image_kind(initramfs, real_root));
            if (initramfs == AdapterId::mkinitfs_busybox)
                validate_mkinitfs_openrc_image_record(record);
            else if (initramfs == AdapterId::mkinitfs_boot_deploy)
                validate_mkinitfs_boot_deploy_image_record(record);
            else
                validate_dracut_systemd_image_record(record);
        }

        std::string serialize_image(const Manifest &manifest) {
            if (!manifest.image)
                return {};
            const auto &image = *manifest.image;
            return std::format("image\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                               image_kind(manifest.initramfs, manifest.real_root), hex(image.kernel_version),
                               hex(image.active_image), image.active_digest, hex(image.candidate_image),
                               image.candidate_digest, image.candidate_bytes, hex(image.known_good_image),
                               image.known_good_digest, hex(image.grub_script_path), image.grub_script_digest,
                               hex(image.grub_config_path), image.bootart_digest);
        }

        DracutSystemdImageRecord parse_image(const std::vector<std::string_view> &item, AdapterId initramfs,
                                             AdapterId real_root) {
            if (item.size() != 14 || item[0] != "image" || item[1] != image_kind(initramfs, real_root))
                throw std::runtime_error("manifest contains an invalid image record");
            DracutSystemdImageRecord image{unhex(item[2]),        unhex(item[3]),       std::string(item[4]),
                                           unhex(item[5]),        std::string(item[6]), number(item[7]),
                                           unhex(item[8]),        std::string(item[9]), unhex(item[10]),
                                           std::string(item[11]), unhex(item[12]),      std::string(item[13])};
            for (const auto &digest : {image.active_digest, image.candidate_digest, image.known_good_digest,
                                       image.grub_script_digest, image.bootart_digest}) {
                if (!digest_text(digest))
                    throw std::runtime_error("manifest image record contains an invalid digest");
            }
            validate_image_record(image, initramfs, real_root);
            return image;
        }

        bool raw_backup_path(std::string_view value, std::optional<std::string_view> transaction = std::nullopt) {
            const auto prefix = std::string(transactions_path) + "/";
            if (!value.starts_with(prefix) || !value.ends_with("/raw-boot-preimage"))
                return false;
            const auto middle = value.substr(prefix.size(), value.size() - prefix.size() - 18);
            return transaction_text(middle) && (!transaction || middle == *transaction);
        }

        bool entry_backup_path(std::string_view value) {
            const auto prefix = std::string(transactions_path) + "/";
            if (!value.starts_with(prefix))
                return false;
            const auto separator = value.find('/', prefix.size());
            if (separator == std::string_view::npos ||
                !transaction_text(value.substr(prefix.size(), separator - prefix.size())))
                return false;
            const auto name = value.substr(separator + 1);
            return name.size() == 13 && name.starts_with("backup-") &&
                   std::ranges::all_of(name.substr(7), [](unsigned char byte) { return byte >= '0' && byte <= '9'; });
        }

        AndroidBootPartitionFact parse_partition(std::span<const std::string_view> item, std::size_t at) {
            if (item.size() < at + 5)
                throw std::runtime_error("state document contains an incomplete raw boot identity");
            AndroidBootPartitionFact partition{unhex(item[at]), unhex(item[at + 1]), number(item[at + 2]),
                                               number(item[at + 3]), std::string(item[at + 4])};
            if (!digest_text(partition.digest) || !safe_android_partition_fact(partition))
                throw std::runtime_error("state document contains an unsafe raw boot identity");
            return partition;
        }

        std::string serialize_raw_manifest(const RawBootManifest &raw) {
            return std::format("raw-boot\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", hex(raw.partition.label),
                               hex(raw.partition.canonical_path), raw.partition.device_number, raw.partition.bytes,
                               raw.original_digest, raw.installed_digest, hex(raw.backup));
        }

        RawBootManifest parse_raw_manifest(const std::vector<std::string_view> &item) {
            if (item.size() != 8 || item[0] != "raw-boot")
                throw std::runtime_error("manifest contains an invalid raw boot record");
            auto partition = parse_partition(item, 1);
            RawBootManifest raw{std::move(partition), std::string(item[5]), std::string(item[6]), unhex(item[7])};
            if (raw.original_digest != raw.partition.digest || !digest_text(raw.installed_digest) ||
                !raw_backup_path(raw.backup)) {
                throw std::runtime_error("manifest contains an inconsistent raw boot record");
            }
            return raw;
        }

        std::string serialize_raw_journal(const RawBootJournal &raw) {
            return std::format("raw-boot\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", progress_name(raw.progress),
                               hex(raw.partition.label), hex(raw.partition.canonical_path), raw.partition.device_number,
                               raw.partition.bytes, raw.original_digest,
                               raw.installed_digest.empty() ? "-" : raw.installed_digest, hex(raw.backup));
        }

        RawBootJournal parse_raw_journal(const std::vector<std::string_view> &item, std::string_view transaction) {
            if (item.size() != 9 || item[0] != "raw-boot")
                throw std::runtime_error("journal contains an invalid raw boot record");
            auto partition = parse_partition(item, 2);
            RawBootJournal raw{std::move(partition), std::string(item[6]),
                               item[7] == "-" ? std::string{} : std::string(item[7]), unhex(item[8]),
                               parse_progress(item[1])};
            if (raw.original_digest != raw.partition.digest ||
                (!raw.installed_digest.empty() && !digest_text(raw.installed_digest)) ||
                !raw_backup_path(raw.backup, transaction)) {
                throw std::runtime_error("journal contains an inconsistent raw boot record");
            }
            return raw;
        }

        std::string serialize_manifest(const Manifest &manifest) {
            std::string output = "BOOTART-MANIFEST\t3\n";
            output += "transaction\t" + manifest.transaction + "\n";
            output += "plan\t" + manifest.plan_id + "\n";
            output +=
                std::format("adapters\t{}\t{}\n", adapter_name(manifest.initramfs), adapter_name(manifest.real_root));
            output += serialize_image(manifest);
            if (manifest.raw_boot)
                output += serialize_raw_manifest(*manifest.raw_boot);
            for (const auto &entry : manifest.entries)
                output += serialize_entry(entry, false);
            for (const auto &directory : manifest.created_dirs)
                output += "dir\t" + hex(directory) + "\n";
            return output;
        }

        Manifest parse_manifest(std::span<const std::byte> bytes) {
            const std::string text(reinterpret_cast<const char *>(bytes.data()), bytes.size());
            if (!text.starts_with("BOOTART-MANIFEST\t3\n"))
                throw std::runtime_error("corrupt installer manifest header");
            Manifest result{};
            bool saw_transaction = false, saw_plan = false, saw_adapters = false;
            std::istringstream input(text.substr(std::string_view("BOOTART-MANIFEST\t3\n").size()));
            std::string line;
            while (std::getline(input, line)) {
                if (line.empty())
                    continue;
                const auto item = fields(line);
                if (item[0] == "transaction" && item.size() == 2 && !saw_transaction) {
                    result.transaction = item[1];
                    saw_transaction = true;
                } else if (item[0] == "plan" && item.size() == 2 && !saw_plan) {
                    result.plan_id = item[1];
                    saw_plan = true;
                } else if (item[0] == "adapters" && item.size() == 3 && !saw_adapters) {
                    result.initramfs = parse_adapter(item[1]);
                    result.real_root = parse_adapter(item[2]);
                    saw_adapters = true;
                } else if (item[0] == "image" && saw_adapters && !result.image) {
                    result.image = parse_image(item, result.initramfs, result.real_root);
                } else if (item[0] == "raw-boot" && !result.raw_boot) {
                    result.raw_boot = parse_raw_manifest(item);
                } else if (item[0] == "entry" && result.entries.size() < max_entries) {
                    result.entries.push_back(parse_entry(item, false));
                } else if (item[0] == "dir" && item.size() == 2 && result.created_dirs.size() < max_entries) {
                    result.created_dirs.push_back(unhex(item[1]));
                } else {
                    throw std::runtime_error("corrupt installer manifest record");
                }
            }
            if (!saw_transaction || !saw_plan || !saw_adapters || result.entries.empty() ||
                !transaction_text(result.transaction) || !digest_text(result.plan_id) ||
                adapter_pair(result.initramfs, result.real_root) == nullptr) {
                throw std::runtime_error("incomplete installer manifest");
            }
            if (result.raw_boot && (!result.image || result.initramfs != AdapterId::mkinitfs_boot_deploy))
                throw std::runtime_error("manifest raw boot record has no boot-deploy image");
            std::set<std::string> paths;
            for (const auto &entry : result.entries) {
                if (entry.kind == EntryKind::transient_file ||
                    (entry.installed_digest.empty() && entry.kind != EntryKind::symlink) ||
                    !paths.insert(entry.path).second) {
                    throw std::runtime_error("manifest contains an invalid or duplicate entry");
                }
                if (!entry.original.backup.empty()) {
                    if (!entry_backup_path(entry.original.backup)) {
                        throw std::runtime_error("manifest backup escapes its transaction");
                    }
                }
            }
            for (const auto &directory : result.created_dirs) {
                host_path("/", directory);
                if (!paths.insert("dir:" + directory).second) {
                    throw std::runtime_error("manifest contains a duplicate directory");
                }
            }
            if (serialize_manifest(result) != text)
                throw std::runtime_error("installer manifest is not canonical");
            return result;
        }

        std::string serialize_journal(const Journal &journal) {
            std::string output = "BOOTART-JOURNAL\t1\n";
            const auto kind = journal.kind == JournalKind::install   ? "install"
                              : journal.kind == JournalKind::refresh ? "refresh"
                                                                     : "uninstall";
            output += std::string("kind\t") + kind + "\n";
            output += "phase\t" + journal.phase + "\n";
            output += "transaction\t" + journal.transaction + "\n";
            if (journal.raw_boot)
                output += serialize_raw_journal(*journal.raw_boot);
            for (const auto &entry : journal.entries)
                output += serialize_entry(entry, true);
            for (const auto &directory : journal.created_dirs)
                output += "dir\t" + hex(directory) + "\n";
            return output;
        }

        Journal parse_journal(std::span<const std::byte> bytes) {
            const std::string text(reinterpret_cast<const char *>(bytes.data()), bytes.size());
            if (!text.starts_with("BOOTART-JOURNAL\t1\n"))
                throw std::runtime_error("corrupt installer journal header");
            Journal result{};
            bool saw_kind = false, saw_phase = false, saw_transaction = false;
            std::istringstream input(text.substr(std::string_view("BOOTART-JOURNAL\t1\n").size()));
            std::string line;
            while (std::getline(input, line)) {
                if (line.empty())
                    continue;
                const auto item = fields(line);
                if (item[0] == "kind" && item.size() == 2 && !saw_kind) {
                    if (item[1] == "install")
                        result.kind = JournalKind::install;
                    else if (item[1] == "refresh")
                        result.kind = JournalKind::refresh;
                    else if (item[1] == "uninstall")
                        result.kind = JournalKind::uninstall;
                    else
                        throw std::runtime_error("corrupt journal kind");
                    saw_kind = true;
                } else if (item[0] == "phase" && item.size() == 2 && !saw_phase) {
                    result.phase = item[1];
                    saw_phase = true;
                } else if (item[0] == "transaction" && item.size() == 2 && !saw_transaction) {
                    result.transaction = item[1];
                    saw_transaction = true;
                } else if (item[0] == "raw-boot" && saw_transaction && !result.raw_boot) {
                    result.raw_boot = parse_raw_journal(item, result.transaction);
                } else if (item[0] == "entry" && result.entries.size() < max_entries) {
                    result.entries.push_back(parse_entry(item, true));
                } else if (item[0] == "dir" && item.size() == 2 && result.created_dirs.size() < max_entries) {
                    result.created_dirs.push_back(unhex(item[1]));
                } else {
                    throw std::runtime_error("corrupt installer journal record");
                }
            }
            const bool valid_phase = result.phase == "bootstrap" || result.phase == "ready" ||
                                     result.phase == "cleanup" || result.phase == "rollback-cleanup";
            if (!saw_kind || !saw_phase || !saw_transaction || result.entries.empty() ||
                !transaction_text(result.transaction) || !valid_phase) {
                throw std::runtime_error("incomplete installer journal");
            }
            std::set<std::string> paths;
            for (const auto &entry : result.entries) {
                if (!paths.insert(entry.path).second) {
                    throw std::runtime_error("journal contains a duplicate entry");
                }
                if (!entry.original.backup.empty()) {
                    const auto prefix = std::string(transactions_path) + "/" + result.transaction + "/backup-";
                    if (!entry.original.backup.starts_with(prefix)) {
                        throw std::runtime_error("journal backup escapes its transaction");
                    }
                }
            }
            for (const auto &directory : result.created_dirs) {
                host_path("/", directory);
                if (!paths.insert("dir:" + directory).second) {
                    throw std::runtime_error("journal contains a duplicate directory");
                }
            }
            if (serialize_journal(result) != text)
                throw std::runtime_error("installer journal is not canonical");
            return result;
        }

        void write_journal(std::string_view root, const Journal &journal) {
            atomic_write_text(host_path(root, journal_path), serialize_journal(journal), 0600);
        }

        void write_manifest(std::string_view root, const Manifest &manifest) {
            atomic_write_text(host_path(root, manifest_path), serialize_manifest(manifest), 0600);
        }

        std::string transaction_id() {
            const auto now = std::chrono::system_clock::now().time_since_epoch().count();
            return std::format("{:x}-{:x}-{:x}", static_cast<std::uint64_t>(now), getpid(),
                               unique_counter.fetch_add(1));
        }

        std::vector<std::string> missing_directories(std::string_view root, std::string_view guest_parent,
                                                     std::uint32_t owner) {
            std::vector<std::string> missing;
            std::filesystem::path relative;
            bool absent_parent = false;
            for (const auto &component : std::filesystem::path(guest_parent).relative_path()) {
                relative /= component;
                const std::string guest = "/" + relative.string();
                if (absent_parent) {
                    missing.push_back(guest);
                    continue;
                }
                bool exists = false;
                const auto path = host_path(root, guest);
                const auto status = lstat_optional(path, exists);
                if (!exists) {
                    absent_parent = true;
                    missing.push_back(guest);
                } else if (!S_ISDIR(status.st_mode) || status.st_uid != owner || (status.st_mode & 0022) != 0) {
                    throw std::runtime_error("installer parent directory is unsafe: " + path.string());
                }
            }
            return missing;
        }

        void create_directories(std::string_view root, const std::vector<std::string> &directories,
                                std::uint32_t owner) {
            for (const auto &guest : directories) {
                const auto path = host_path(root, guest);
                const auto mode =
                    guest.starts_with("/var/lib/bootart/install") || guest.starts_with("/boot/.bootart-candidate")
                        ? 0700
                        : 0755;
                if (mkdir(path.c_str(), mode) != 0 && errno != EEXIST)
                    throw system_error("create directory", path);
                validate_directory(path, owner);
                fsync_directory(path.parent_path());
            }
        }

        void unlink_file_or_link(const std::filesystem::path &path) {
            bool exists = false;
            const auto status = lstat_optional(path, exists);
            if (!exists)
                return;
            if (!S_ISREG(status.st_mode) && !S_ISLNK(status.st_mode)) {
                throw std::runtime_error("refusing to remove a non-file installer target: " + path.string());
            }
            if (unlink(path.c_str()) != 0)
                throw system_error("remove", path);
            fsync_directory(path.parent_path());
        }

        Preimage capture_preimage(std::string_view root, std::string_view guest, std::uint32_t owner) {
            const auto path = host_path(root, guest);
            bool exists = false;
            const auto status = lstat_optional(path, exists);
            if (!exists)
                return {};
            if (status.st_uid != owner)
                throw std::runtime_error("installer target is not owned by the expected uid: " + path.string());
            if (S_ISREG(status.st_mode)) {
                Preimage preimage;
                preimage.kind = PreimageKind::file;
                preimage.captured = read_regular(path, owner, &preimage.mode);
                preimage.digest = sha256(preimage.captured);
                return preimage;
            }
            if (S_ISLNK(status.st_mode)) {
                std::array<char, 4097> buffer{};
                const auto count = readlink(path.c_str(), buffer.data(), buffer.size() - 1);
                if (count < 0 || static_cast<std::size_t>(count) >= buffer.size() - 1) {
                    throw system_error("read symlink", path);
                }
                Preimage preimage;
                preimage.kind = PreimageKind::symlink;
                preimage.target.assign(buffer.data(), static_cast<std::size_t>(count));
                return preimage;
            }
            throw std::runtime_error("installer target has an unsupported type: " + path.string());
        }

        void store_backups(std::string_view root, Journal &journal, std::uint32_t owner) {
            const auto transaction_dir = std::string(transactions_path) + "/" + journal.transaction;
            for (std::size_t index = 0; index < journal.entries.size(); ++index) {
                auto &preimage = journal.entries[index].original;
                if (preimage.kind != PreimageKind::file)
                    continue;
                preimage.backup = std::format("{}/backup-{:06}", transaction_dir, index);
                const auto path = host_path(root, preimage.backup);
                atomic_write(path, preimage.captured, 0600);
                const auto verified = read_regular(path, owner, nullptr, max_transaction_bytes);
                if (sha256(verified) != preimage.digest)
                    throw std::runtime_error("transaction backup digest mismatch");
                preimage.captured.clear();
                preimage.captured.shrink_to_fit();
            }
        }

        void store_raw_backup(std::string_view root, Journal &journal, std::span<const std::byte> contents,
                              std::uint32_t owner) {
            if (!journal.raw_boot)
                throw std::runtime_error("raw boot backup has no journal record");
            auto &raw = *journal.raw_boot;
            if (contents.size() != raw.partition.bytes || sha256(contents) != raw.original_digest ||
                !raw_backup_path(raw.backup, journal.transaction)) {
                throw std::runtime_error("raw boot preimage differs from the journal identity");
            }
            const auto path = host_path(root, raw.backup);
            atomic_write(path, contents, 0600);
            std::uint16_t mode{};
            const auto verified = read_regular(path, owner, &mode, max_transaction_bytes);
            if (mode != 0600 || verified.size() != contents.size() || sha256(verified) != raw.original_digest)
                throw std::runtime_error("raw boot backup verification failed");
        }

        std::vector<std::byte> read_raw_backup(std::string_view root, const RawBootJournal &raw, std::uint32_t owner) {
            if (!raw_backup_path(raw.backup))
                throw std::runtime_error("raw boot backup path is unsafe");
            std::uint16_t mode{};
            auto contents = read_regular(host_path(root, raw.backup), owner, &mode, max_transaction_bytes);
            if (mode != 0600 || contents.size() != raw.partition.bytes || sha256(contents) != raw.original_digest ||
                raw.original_digest != raw.partition.digest) {
                throw std::runtime_error("raw boot backup verification failed");
            }
            return contents;
        }

        std::vector<std::byte> read_raw_backup(std::string_view root, const RawBootManifest &raw, std::uint32_t owner) {
            return read_raw_backup(
                root,
                RawBootJournal{raw.partition, raw.original_digest, raw.installed_digest, raw.backup, Progress::applied},
                owner);
        }

        void restore_preimage(std::string_view root, const Entry &entry, std::uint32_t owner) {
            const auto path = host_path(root, entry.path);
            switch (entry.original.kind) {
            case PreimageKind::absent:
                unlink_file_or_link(path);
                break;
            case PreimageKind::file: {
                const auto backup =
                    read_regular(host_path(root, entry.original.backup), owner, nullptr, max_transaction_bytes);
                if (sha256(backup) != entry.original.digest)
                    throw std::runtime_error("transaction backup digest mismatch");
                atomic_write(path, backup, entry.original.mode);
                break;
            }
            case PreimageKind::symlink:
                atomic_symlink(path, entry.original.target);
                break;
            }
        }

        std::vector<std::string> remove_empty_directories(std::string_view root, std::vector<std::string> directories) {
            std::ranges::sort(directories, [](const auto &left, const auto &right) {
                return std::count(left.begin(), left.end(), '/') > std::count(right.begin(), right.end(), '/');
            });
            directories.erase(std::unique(directories.begin(), directories.end()), directories.end());
            std::vector<std::string> preserved;
            for (const auto &guest : directories) {
                const auto path = host_path(root, guest);
                if (rmdir(path.c_str()) != 0) {
                    if (errno == ENOENT)
                        continue;
                    if (errno == ENOTEMPTY || errno == EEXIST) {
                        preserved.push_back(guest);
                        continue;
                    }
                    throw system_error("remove directory", path);
                }
                fsync_directory(path.parent_path());
            }
            return preserved;
        }

        void cleanup_transaction_backups(std::string_view root, const Journal &journal) {
            const auto transaction_directory =
                host_path(root, std::string(transactions_path) + "/" + journal.transaction);
            const auto inspection = transaction_directory / "unpacked-candidate";
            bool inspection_exists = false;
            const auto inspection_status = lstat_optional(inspection, inspection_exists);
            if (inspection_exists) {
                if (!S_ISDIR(inspection_status.st_mode) || S_ISLNK(inspection_status.st_mode) ||
                    inspection_status.st_uid != 0 || (inspection_status.st_mode & 0077) != 0) {
                    throw std::runtime_error("unsafe unpacked-candidate cleanup directory");
                }
                std::error_code error;
                std::filesystem::remove_all(inspection, error);
                if (error)
                    throw std::runtime_error("remove unpacked-candidate tree: " + error.message());
            }
            for (const auto &entry : journal.entries) {
                if (!entry.original.backup.empty())
                    unlink_file_or_link(host_path(root, entry.original.backup));
            }
            if (journal.raw_boot)
                unlink_file_or_link(host_path(root, journal.raw_boot->backup));
            if (rmdir(transaction_directory.c_str()) != 0 && errno != ENOENT && errno != ENOTEMPTY) {
                throw system_error("remove transaction directory", transaction_directory);
            }
        }

        void prune_boot_deploy_candidate(std::string_view root, std::string_view candidate_directory,
                                         std::span<const std::string> keep, std::uint32_t owner) {
            if (candidate_directory != "/boot/.bootart-candidate")
                throw std::runtime_error("boot-deploy candidate directory differs from the fixed contract");
            const auto directory = host_path(root, candidate_directory);
            validate_directory(directory, owner);
            std::set<std::filesystem::path> kept;
            for (const auto &guest : keep) {
                if (!guest.starts_with(std::string(candidate_directory) + "/"))
                    throw std::runtime_error("boot-deploy candidate keep path escapes its directory");
                kept.insert(host_path(root, guest));
            }
            std::vector<std::filesystem::path> directories;
            std::size_t count = 0;
            for (const auto &entry : std::filesystem::recursive_directory_iterator(directory)) {
                if (++count > max_archive_entries)
                    throw std::runtime_error("boot-deploy candidate tree exceeds its entry bound");
                struct stat status{};
                if (lstat(entry.path().c_str(), &status) != 0)
                    throw system_error("inspect boot-deploy candidate", entry.path());
                if (status.st_uid != owner || (status.st_mode & 0022) != 0 || S_ISLNK(status.st_mode))
                    throw std::runtime_error("boot-deploy candidate tree contains an unsafe entry");
                if (S_ISDIR(status.st_mode)) {
                    directories.push_back(entry.path());
                } else if (!S_ISREG(status.st_mode) || status.st_nlink != 1) {
                    throw std::runtime_error("boot-deploy candidate tree contains an unsupported entry");
                } else if (!kept.contains(entry.path())) {
                    unlink_file_or_link(entry.path());
                }
            }
            std::ranges::sort(directories, [](const auto &left, const auto &right) {
                return std::distance(left.begin(), left.end()) > std::distance(right.begin(), right.end());
            });
            for (const auto &path : directories) {
                if (rmdir(path.c_str()) != 0 && errno != ENOENT)
                    throw system_error("remove boot-deploy candidate directory", path);
            }
        }

        void cleanup_manifest_backups(std::string_view root, const Manifest &manifest) {
            std::set<std::string> directories;
            for (const auto &entry : manifest.entries) {
                if (entry.original.backup.empty())
                    continue;
                unlink_file_or_link(host_path(root, entry.original.backup));
                directories.insert(std::filesystem::path(entry.original.backup).parent_path().string());
            }
            if (manifest.raw_boot) {
                unlink_file_or_link(host_path(root, manifest.raw_boot->backup));
                directories.insert(std::filesystem::path(manifest.raw_boot->backup).parent_path().string());
            }
            for (const auto &directory : directories) {
                const auto host = host_path(root, directory);
                if (rmdir(host.c_str()) != 0 && errno != ENOENT)
                    throw system_error("remove installed transaction directory", host);
            }
        }

        void rollback(std::string_view root, Journal &journal, std::uint32_t owner) {
            for (auto iterator = journal.entries.rbegin(); iterator != journal.entries.rend(); ++iterator) {
                if (iterator->progress != Progress::planned)
                    restore_preimage(root, *iterator, owner);
            }
            if (journal.raw_boot && journal.raw_boot->progress != Progress::planned) {
                const auto original = read_raw_backup(root, *journal.raw_boot, owner);
                restore_android_boot_partition_durable(journal.raw_boot->partition, original);
            }
            journal.phase = "rollback-cleanup";
            write_journal(root, journal);
            cleanup_transaction_backups(root, journal);
            static_cast<void>(remove_empty_directories(root, journal.created_dirs));
            unlink_file_or_link(host_path(root, journal_path));
        }

        void verify_expected_current(std::string_view root, const Entry &entry, std::uint32_t owner) {
            const auto path = host_path(root, entry.path);
            bool exists = false;
            const auto status = lstat_optional(path, exists);
            if (entry.kind == EntryKind::symlink) {
                if (!exists || !S_ISLNK(status.st_mode))
                    throw std::runtime_error("managed symlink changed: " + entry.path);
                std::array<char, 4097> buffer{};
                const auto count = readlink(path.c_str(), buffer.data(), buffer.size() - 1);
                if (count < 0 ||
                    std::string_view(buffer.data(), static_cast<std::size_t>(count)) != entry.installed_target) {
                    throw std::runtime_error("managed symlink changed: " + entry.path);
                }
                return;
            }
            std::uint16_t mode = 0;
            const auto bytes = read_regular(path, owner, &mode, max_candidate_bytes);
            if (mode != entry.installed_mode || sha256(bytes) != entry.installed_digest) {
                throw std::runtime_error("managed file changed: " + entry.path);
            }
        }

        void apply_entry(std::string_view root, const Entry &entry, std::uint32_t owner) {
            const auto path = host_path(root, entry.path);
            if (entry.original.kind == PreimageKind::absent) {
                bool exists = false;
                static_cast<void>(lstat_optional(path, exists));
                if (exists)
                    throw std::runtime_error("installer destination collision: " + entry.path);
            } else {
                const auto current = capture_preimage(root, entry.path, owner);
                if (current.kind != entry.original.kind || current.mode != entry.original.mode ||
                    current.digest != entry.original.digest || current.target != entry.original.target) {
                    throw std::runtime_error("installer destination changed after preimage capture: " + entry.path);
                }
            }
            if (entry.kind == EntryKind::symlink)
                atomic_symlink(path, entry.installed_target);
            else
                atomic_write(path, entry.content, entry.installed_mode);
        }

        std::vector<Entry> build_actions(std::string_view root, const InstallPlan &plan, std::uint32_t owner) {
            std::vector<Entry> entries;
            for (const auto &operation : plan.operations) {
                Entry entry;
                entry.kind = EntryKind::file;
                entry.path = operation.path;
                entry.installed_mode = operation.mode;
                entry.installed_digest = operation.digest;
                entry.content = operation.content;
                entry.original = capture_preimage(root, entry.path, owner);
                if (entry.original.kind != PreimageKind::absent) {
                    throw std::runtime_error("unowned install destination already exists: " + entry.path);
                }
                entries.push_back(std::move(entry));
            }
            std::map<std::string, std::vector<const ManagedSnippetOperation *>> snippets;
            for (const auto &operation : plan.managed_snippets)
                snippets[operation.target].push_back(&operation);
            for (const auto &[target, operations] : snippets) {
                auto original = capture_preimage(root, target, owner);
                if (original.kind != PreimageKind::file)
                    throw std::runtime_error("managed snippet target is not a regular file: " + target);
                std::string text(reinterpret_cast<const char *>(original.captured.data()), original.captured.size());
                for (const auto *operation : operations) {
                    if (operation->adapter == AdapterId::mkinitfs_busybox) {
                        const auto patched = integration::patch_mkinitfs_init(text);
                        if (!patched)
                            throw std::runtime_error("mkinitfs managed snippet contract mismatch");
                        text = *patched;
                    } else if (operation->adapter == AdapterId::mkinitfs_boot_deploy) {
                        const auto patched = integration::patch_boot_deploy_init_functions(
                            text, integration::reviewed_boot_deploy_initramfs_version);
                        if (!patched)
                            throw std::runtime_error("boot-deploy managed snippet contract mismatch");
                        text = *patched;
                    } else {
                        throw std::runtime_error("unsupported managed snippet adapter");
                    }
                }
                Entry entry;
                entry.kind = EntryKind::patched_file;
                entry.path = target;
                entry.installed_mode = original.mode;
                entry.content.assign(reinterpret_cast<const std::byte *>(text.data()),
                                     reinterpret_cast<const std::byte *>(text.data() + text.size()));
                entry.installed_digest = sha256(entry.content);
                entry.original = std::move(original);
                entries.push_back(std::move(entry));
            }
            for (const auto &operation : plan.activations) {
                if (operation.scope != ActivationScope::real_root)
                    continue;
                Entry entry;
                entry.kind = EntryKind::symlink;
                entry.path = operation.path;
                entry.installed_target = operation.relative_target;
                entry.original = capture_preimage(root, entry.path, owner);
                if (entry.original.kind != PreimageKind::absent) {
                    throw std::runtime_error("unowned activation destination already exists: " + entry.path);
                }
                entries.push_back(std::move(entry));
            }
            std::ranges::sort(entries, {}, &Entry::path);
            return entries;
        }

        std::vector<std::string> required_missing_directories(std::string_view root, const std::vector<Entry> &entries,
                                                              std::string_view transaction, std::uint32_t owner) {
            std::set<std::string> result;
            for (const auto &entry : entries) {
                for (auto &directory :
                     missing_directories(root, std::filesystem::path(entry.path).parent_path().string(), owner)) {
                    result.insert(std::move(directory));
                }
            }
            for (const auto state : {std::string_view("/var/lib/bootart"), std::string_view("/var/lib/bootart/install"),
                                     transactions_path}) {
                for (auto &directory : missing_directories(root, state, owner))
                    result.insert(std::move(directory));
                bool exists = false;
                static_cast<void>(lstat_optional(host_path(root, state), exists));
                if (!exists)
                    result.insert(std::string(state));
            }
            result.insert(std::string(transactions_path) + "/" + std::string(transaction));
            return {result.begin(), result.end()};
        }

        std::optional<Manifest> read_manifest(std::string_view root, std::uint32_t owner) {
            const auto document = read_optional_document(root, manifest_path, owner);
            if (!document)
                return std::nullopt;
            return parse_manifest(*document);
        }

        std::optional<Journal> read_journal(std::string_view root, std::uint32_t owner) {
            const auto document = read_optional_document(root, journal_path, owner);
            if (!document)
                return std::nullopt;
            return parse_journal(*document);
        }

        FileStatusState file_status(std::string_view root, const Entry &entry, std::uint32_t owner,
                                    std::string &actual) {
            const auto path = host_path(root, entry.path);
            bool exists = false;
            const auto status = lstat_optional(path, exists);
            if (!exists)
                return FileStatusState::missing;
            if (entry.kind == EntryKind::symlink) {
                if (!S_ISLNK(status.st_mode))
                    return FileStatusState::type_modified;
                std::array<char, 4097> buffer{};
                const auto count = readlink(path.c_str(), buffer.data(), buffer.size() - 1);
                if (count < 0)
                    return FileStatusState::type_modified;
                actual.assign(buffer.data(), static_cast<std::size_t>(count));
                return actual == entry.installed_target ? FileStatusState::exact : FileStatusState::content_modified;
            }
            if (!S_ISREG(status.st_mode))
                return FileStatusState::type_modified;
            try {
                std::uint16_t mode = 0;
                const auto bytes = read_regular(path, owner, &mode, max_candidate_bytes);
                actual = sha256(bytes);
                if (mode != entry.installed_mode)
                    return FileStatusState::mode_modified;
                return actual == entry.installed_digest ? FileStatusState::exact : FileStatusState::content_modified;
            } catch (...) {
                return FileStatusState::type_modified;
            }
        }

        std::string read_hostname() {
            std::array<char, 256> buffer{};
            if (gethostname(buffer.data(), buffer.size()) != 0)
                throw system_error("read current hostname", "/proc/sys/kernel/hostname");
            const auto terminator = std::ranges::find(buffer, '\0');
            if (terminator == buffer.end())
                throw std::runtime_error("current hostname exceeds its size bound");
            const std::string hostname(buffer.begin(), terminator);
            if (hostname.empty() || hostname.size() > 255 || hostname.find_first_of(" \t\r\n") != std::string::npos) {
                throw std::runtime_error("current hostname is not canonical");
            }
            return hostname;
        }

        std::vector<std::byte> read_authorization_file(const std::filesystem::path &path,
                                                       std::optional<std::uint16_t> expected_mode, std::size_t limit) {
            std::uint16_t mode{};
            auto bytes = read_regular(path, 0, &mode, limit);
            if ((mode & 0022) != 0 || (expected_mode && mode != *expected_mode)) {
                throw std::runtime_error("package-hook authorization file has an unsafe mode: " + path.string());
            }
            return bytes;
        }

        std::vector<std::byte> read_proc_file(pid_t process, std::string_view name, std::size_t limit) {
            if (process <= 1 || name.empty() || name.contains('/'))
                throw std::runtime_error("package-hook process identity is invalid");
            const auto path = std::filesystem::path("/proc") / std::to_string(process) / name;
            const int descriptor = open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
            if (descriptor < 0)
                throw system_error("open package-hook process metadata", path);
            struct stat status{};
            if (fstat(descriptor, &status) != 0 || !S_ISREG(status.st_mode) || status.st_uid != 0 ||
                (status.st_mode & 0022) != 0) {
                close(descriptor);
                throw std::runtime_error("package-hook process metadata is unsafe: " + path.string());
            }
            std::vector<std::byte> bytes;
            std::array<std::byte, 4096> buffer{};
            while (true) {
                const auto count = read(descriptor, buffer.data(), buffer.size());
                if (count < 0 && errno == EINTR)
                    continue;
                if (count < 0) {
                    const auto error = system_error("read package-hook process metadata", path);
                    close(descriptor);
                    throw error;
                }
                if (count == 0)
                    break;
                if (bytes.size() + static_cast<std::size_t>(count) > limit) {
                    close(descriptor);
                    throw std::runtime_error("package-hook process metadata exceeds its size bound");
                }
                bytes.insert(bytes.end(), buffer.begin(), buffer.begin() + count);
            }
            if (close(descriptor) != 0)
                throw system_error("close package-hook process metadata", path);
            return bytes;
        }

        std::string proc_text(pid_t process, std::string_view name, std::size_t limit) {
            const auto bytes = read_proc_file(process, name, limit);
            if (std::ranges::find(bytes, std::byte{}) != bytes.end())
                throw std::runtime_error("package-hook process text contains NUL");
            return {reinterpret_cast<const char *>(bytes.data()), bytes.size()};
        }

        pid_t proc_parent(pid_t process) {
            const auto status = proc_text(process, "status", 64 * 1024);
            std::optional<pid_t> parent;
            std::istringstream input(status);
            std::string line;
            while (std::getline(input, line)) {
                if (!line.starts_with("PPid:\t"))
                    continue;
                if (parent || line.size() == 6)
                    throw std::runtime_error("package-hook process status has an invalid parent");
                const auto parsed = number(std::string_view(line).substr(6));
                if (parsed > static_cast<std::uint64_t>(std::numeric_limits<pid_t>::max()))
                    throw std::runtime_error("package-hook parent process is out of range");
                parent = static_cast<pid_t>(parsed);
            }
            if (!parent)
                throw std::runtime_error("package-hook process status has no parent");
            return *parent;
        }

        std::string proc_comm(pid_t process) {
            auto comm = proc_text(process, "comm", 256);
            while (!comm.empty() && (comm.back() == '\n' || comm.back() == '\r'))
                comm.pop_back();
            if (comm.empty() || comm.contains('\n') || comm.contains('\r'))
                throw std::runtime_error("package-hook process name is malformed");
            return comm;
        }

        std::uint64_t proc_start_time(pid_t process) {
            const auto stat = proc_text(process, "stat", 4096);
            const auto command_end = stat.rfind(") ");
            if (command_end == std::string::npos)
                throw std::runtime_error("package-hook process stat is malformed");
            std::istringstream input(stat.substr(command_end + 2));
            std::string field;
            for (std::size_t index = 0; index <= 19; ++index) {
                if (!(input >> field))
                    throw std::runtime_error("package-hook process stat has no start time");
            }
            return number(field);
        }

        void validate_process_executable(pid_t process) {
            const auto link = std::filesystem::path("/proc") / std::to_string(process) / "exe";
            std::array<char, 4097> target{};
            const auto count = readlink(link.c_str(), target.data(), target.size() - 1);
            if (count <= 0 || static_cast<std::size_t>(count) == target.size() - 1)
                throw std::runtime_error("package-hook process executable cannot be resolved");
            const std::filesystem::path path(std::string(target.data(), static_cast<std::size_t>(count)));
            if (!path.is_absolute())
                throw std::runtime_error("package-hook process executable is not absolute");
            static_cast<void>(read_authorization_file(path, std::nullopt, max_install_file_bytes));
        }

        bool hook_argument(std::span<const std::byte> argument) {
            constexpr std::string_view absolute = "/etc/apk/commit_hooks.d/95-bootart-raw-boot";
            constexpr std::string_view relative = "etc/apk/commit_hooks.d/95-bootart-raw-boot";
            const std::string_view value(reinterpret_cast<const char *>(argument.data()), argument.size());
            return value == absolute || value == relative;
        }

        void validate_hook_shell(pid_t process) {
            const auto bytes = read_proc_file(process, "cmdline", 4096);
            if (bytes.empty() || bytes.back() != std::byte{})
                throw std::runtime_error("package-hook shell command line is malformed");
            std::vector<std::span<const std::byte>> arguments;
            std::size_t first = 0;
            for (std::size_t index = 0; index < bytes.size(); ++index) {
                if (bytes[index] != std::byte{})
                    continue;
                arguments.emplace_back(bytes.data() + first, index - first);
                first = index + 1;
            }
            const auto text = [](std::span<const std::byte> value) {
                return std::string_view(reinterpret_cast<const char *>(value.data()), value.size());
            };
            const bool interpreter =
                arguments.size() == 3 &&
                (text(arguments[0]) == "/bin/sh" || text(arguments[0]) == "/bin/ash" || text(arguments[0]) == "sh" ||
                 text(arguments[0]) == "ash" || text(arguments[0]) == "/bin/busybox") &&
                hook_argument(arguments[1]) && text(arguments[2]) == "post-commit";
            const bool script_title =
                arguments.size() == 2 && hook_argument(arguments[0]) && text(arguments[1]) == "post-commit";
            if (!interpreter && !script_title)
                throw std::runtime_error("direct parent is not the exact installed apk commit hook");
        }

        void authorize_package_hook() {
            constexpr std::string_view hook_path = "/etc/apk/commit_hooks.d/95-bootart-raw-boot";
            const auto hook = read_authorization_file(hook_path, 0755, 64 * 1024);
            const auto expected_hook = std::as_bytes(std::span(integration::mkinitfs_boot_deploy::apk_commit_hook));
            if (!std::ranges::equal(hook, expected_hook))
                throw std::runtime_error("installed apk commit hook differs from the embedded hook");

            const auto installed = read_authorization_file("/usr/bin/bootart", 0755, max_install_file_bytes);
            const auto running = read_running_elf();
            if (sha256(installed) != sha256(running))
                throw std::runtime_error("running Bootart ELF differs from the installed ELF");

            const auto shell = getppid();
            if (shell <= 1)
                throw std::runtime_error("package-hook parent is not a user process");
            const auto shell_start = proc_start_time(shell);
            validate_hook_shell(shell);
            validate_process_executable(shell);
            const auto apk = proc_parent(shell);
            if (apk <= 1 || proc_comm(apk) != "apk")
                throw std::runtime_error("package-hook shell parent is not apk");
            const auto apk_start = proc_start_time(apk);
            validate_process_executable(apk);
            if (getppid() != shell || proc_start_time(shell) != shell_start || proc_parent(shell) != apk ||
                proc_start_time(apk) != apk_start || proc_comm(apk) != "apk") {
                throw std::runtime_error("package-hook process ancestry changed during authorization");
            }
            validate_hook_shell(shell);
        }

        void validate_plan_structure(const InstallPlan &plan) {
            const PlanOperation *binary = nullptr;
            for (const auto &operation : plan.operations) {
                if (operation.path == "/usr/bin/bootart" && operation.source.kind == PlanSourceKind::bootart_elf) {
                    if (binary != nullptr)
                        throw std::runtime_error("install plan contains duplicate Bootart ELF operations");
                    binary = &operation;
                }
            }
            if (binary == nullptr)
                throw std::runtime_error("install plan has no Bootart ELF operation");
            const auto *pair = adapter_pair(plan.initramfs, plan.real_root);
            if (pair == nullptr)
                throw std::runtime_error("install plan has an unknown adapter pair");
            const auto expected = build_install_plan(binary->content, plan.initramfs, plan.real_root,
                                                     pair->status == SupportStatus::experimental_unproven, plan.root);
            const auto operation_equal = [](const auto &left, const auto &right) {
                return left.path == right.path && left.mode == right.mode && left.owner_uid == right.owner_uid &&
                       left.digest == right.digest && left.source.kind == right.source.kind &&
                       left.source.template_id == right.source.template_id && left.content == right.content;
            };
            const auto snippet_equal = [](const auto &left, const auto &right) {
                return left.adapter == right.adapter && left.target == right.target &&
                       left.insertion_point == right.insertion_point && left.digest == right.digest &&
                       left.source == right.source;
            };
            const auto activation_equal = [](const auto &left, const auto &right) {
                return left.adapter == right.adapter && left.scope == right.scope && left.relation == right.relation &&
                       left.path == right.path && left.relative_target == right.relative_target &&
                       left.owner_uid == right.owner_uid && left.source == right.source &&
                       left.runlevel == right.runlevel;
            };
            if (plan.operations.size() != expected.operations.size() ||
                !std::equal(plan.operations.begin(), plan.operations.end(), expected.operations.begin(),
                            operation_equal) ||
                plan.managed_snippets.size() != expected.managed_snippets.size() ||
                !std::equal(plan.managed_snippets.begin(), plan.managed_snippets.end(),
                            expected.managed_snippets.begin(), snippet_equal) ||
                plan.activations.size() != expected.activations.size() ||
                !std::equal(plan.activations.begin(), plan.activations.end(), expected.activations.begin(),
                            activation_equal)) {
                throw std::runtime_error("install plan differs from the canonical embedded inventory");
            }
        }

    } // namespace

    std::optional<AndroidBootPartitionFact> installed_android_boot_partition() {
        if (read_journal("/", 0))
            throw std::runtime_error("explicit recovery is required before Android boot discovery");
        const auto manifest = read_manifest("/", 0);
        if (!manifest || !manifest->raw_boot)
            return std::nullopt;
        auto partition = manifest->raw_boot->partition;
        const auto current = read_android_boot_partition(partition, false);
        if (sha256(current) != manifest->raw_boot->installed_digest)
            throw std::runtime_error("managed Android boot partition changed");
        partition.digest = manifest->raw_boot->installed_digest;
        return partition;
    }

    Installer::Installer(std::string root, std::uint32_t expected_owner_uid, bool mutation_unlocked)
        : root_(std::move(root)), expected_owner_uid_(expected_owner_uid), mutation_unlocked_(mutation_unlocked) {
        validate_root(root_, expected_owner_uid_);
    }

    const std::string &Installer::root() const noexcept { return root_; }

    Installer Installer::live_root_read_only() { return Installer("/", 0, false); }

    Installer Installer::live_root_mutating(std::string_view confirmation, bool package_hook) {
        if (geteuid() != 0)
            throw std::runtime_error("installer mutation requires effective uid 0");
        if (confirmation.empty() || confirmation != read_hostname()) {
            throw std::runtime_error("confirmation does not equal the exact current hostname");
        }
        if (package_hook) {
            authorize_package_hook();
        } else if (!isatty(STDIN_FILENO) || !isatty(STDOUT_FILENO)) {
            throw std::runtime_error("installer mutation requires interactive standard input and output");
        }
        return Installer("/", 0, true);
    }

    StatusReport Installer::status() const {
        validate_root(root_, expected_owner_uid_);
        StatusReport report{};
        report.recovery_required = read_journal(root_, expected_owner_uid_).has_value();
        const auto manifest = read_manifest(root_, expected_owner_uid_);
        if (!manifest)
            return report;
        report.installed = true;
        report.transaction = manifest->transaction;
        for (const auto &entry : manifest->entries) {
            InstalledFileStatus status;
            status.path = entry.path;
            status.expected_mode = entry.installed_mode;
            status.expected_digest = entry.kind == EntryKind::symlink ? entry.installed_target : entry.installed_digest;
            status.state = file_status(root_, entry, expected_owner_uid_, status.actual_digest);
            report.files.push_back(std::move(status));
        }
        if (!manifest->image) {
            report.image_verification = "unresolved blocker=manifest-has-no-image-record";
        } else {
            const auto &image = *manifest->image;
            std::vector<std::string> modified;
            for (const auto &[path, digest] : std::array{std::pair{image.active_image, image.active_digest},
                                                         std::pair{image.known_good_image, image.known_good_digest},
                                                         std::pair{image.grub_script_path, image.grub_script_digest}}) {
                try {
                    if (sha256(read_regular(host_path(root_, path), expected_owner_uid_, nullptr,
                                            max_candidate_bytes)) != digest)
                        modified.push_back(path);
                } catch (...) {
                    modified.push_back(path);
                }
            }
            bool candidate_exists = false;
            static_cast<void>(lstat_optional(host_path(root_, image.candidate_image), candidate_exists));
            if (candidate_exists)
                modified.push_back(image.candidate_image);
            if (manifest->raw_boot) {
                const auto &raw = *manifest->raw_boot;
                try {
                    if (sha256(read_android_boot_partition(raw.partition, false)) != raw.installed_digest)
                        modified.push_back(raw.partition.canonical_path);
                } catch (...) {
                    modified.push_back(raw.partition.canonical_path);
                }
                try {
                    static_cast<void>(read_raw_backup(root_, raw, expected_owner_uid_));
                } catch (...) {
                    modified.push_back(raw.backup);
                }
            }
            std::ranges::sort(modified);
            modified.erase(std::unique(modified.begin(), modified.end()), modified.end());
            if (modified.empty()) {
                report.image_verification =
                    std::format("verified active-sha256={} known-good-sha256={} bootart-sha256={}", image.active_digest,
                                image.known_good_digest, image.bootart_digest);
            } else {
                report.image_verification = "modified paths=";
                for (std::size_t index = 0; index < modified.size(); ++index) {
                    if (index != 0)
                        report.image_verification += ',';
                    report.image_verification += modified[index];
                }
            }
        }
        return report;
    }

    ApplyOutcome Installer::apply(const InstallPlan &plan) {
        if (!mutation_unlocked_)
            throw std::runtime_error("installer mutation is locked");
        if (geteuid() != expected_owner_uid_)
            throw std::runtime_error("installer mutation uid mismatch");
        if (plan.root != root_)
            throw std::runtime_error("install plan root mismatch");
        validate_plan_structure(plan);
        auto lock = lock_root(root_, expected_owner_uid_);
        if (read_journal(root_, expected_owner_uid_))
            throw std::runtime_error("explicit recovery is required");
        if (const auto existing = read_manifest(root_, expected_owner_uid_)) {
            if (existing->plan_id != plan.identity() || existing->initramfs != plan.initramfs ||
                existing->real_root != plan.real_root) {
                throw std::runtime_error("an installation with a different plan already exists");
            }
            for (const auto &entry : existing->entries)
                verify_expected_current(root_, entry, expected_owner_uid_);
            return ApplyOutcome::already_current;
        }

        Journal journal{
            JournalKind::install, "bootstrap", transaction_id(), build_actions(root_, plan, expected_owner_uid_), {},
            std::nullopt};
        journal.created_dirs =
            required_missing_directories(root_, journal.entries, journal.transaction, expected_owner_uid_);
        write_journal(root_, journal);
        bool manifest_committed = false;
        try {
            create_directories(root_, journal.created_dirs, expected_owner_uid_);
            store_backups(root_, journal, expected_owner_uid_);
            journal.phase = "ready";
            write_journal(root_, journal);
            for (std::size_t index = 0; index < journal.entries.size(); ++index) {
                journal.entries[index].progress = Progress::in_progress;
                write_journal(root_, journal);
                apply_entry(root_, journal.entries[index], expected_owner_uid_);
                journal.entries[index].progress = Progress::applied;
                write_journal(root_, journal);
            }
            Manifest manifest{journal.transaction, plan.identity(), plan.initramfs,  plan.real_root,
                              std::nullopt,        std::nullopt,    journal.entries, journal.created_dirs};
            for (auto &entry : manifest.entries) {
                entry.progress = Progress::planned;
                entry.content.clear();
                entry.original.captured.clear();
            }
            write_manifest(root_, manifest);
            manifest_committed = true;
            journal.phase = "cleanup";
            write_journal(root_, journal);
            unlink_file_or_link(host_path(root_, journal_path));
            return ApplyOutcome::installed;
        } catch (...) {
            const auto failure = std::current_exception();
            if (!manifest_committed) {
                try {
                    rollback(root_, journal, expected_owner_uid_);
                } catch (...) {
                }
            }
            std::rethrow_exception(failure);
        }
    }

    ApplyOutcome Installer::apply_exact(const InstallPlan &plan, const ExactInstallDiscovery &discovery) {
        if (!mutation_unlocked_)
            throw std::runtime_error("installer mutation is locked");
        if (geteuid() != expected_owner_uid_)
            throw std::runtime_error("installer mutation uid mismatch");
        if (root_ != "/" || plan.root != root_) {
            throw std::runtime_error("exact installer mutation requires the live root");
        }
        if (plan.initramfs != discovery.initramfs || plan.real_root != discovery.real_root) {
            throw std::runtime_error("exact backend differs from the install plan");
        }
        validate_plan_structure(plan);
        auto lock = lock_root(root_, expected_owner_uid_);
        if (read_journal(root_, expected_owner_uid_))
            throw std::runtime_error("explicit recovery is required");

        const auto active_path =
            std::visit([](const auto &contract) { return contract.active_image; }, discovery.contract);
        const auto candidate_path =
            std::visit([](const auto &contract) { return contract.candidate_image; }, discovery.contract);
        const auto known_good_path =
            std::visit([](const auto &contract) { return contract.known_good_image; }, discovery.contract);
        const auto known_good_digest =
            std::visit([](const auto &contract) { return contract.known_good_digest; }, discovery.contract);
        const auto generate = std::visit([](const auto &contract) { return contract.generate; }, discovery.contract);

        std::string boot_entry_path;
        std::uint16_t boot_entry_mode{};
        std::vector<std::byte> boot_entry_content;
        std::optional<std::string> boot_config_path;
        std::optional<GeneratorRequest> boot_update;
        std::optional<std::tuple<std::string, std::uint16_t, std::vector<std::byte>, std::vector<std::byte>>>
            configuration;
        std::optional<std::tuple<std::string, std::uint16_t, std::vector<std::byte>, std::vector<std::byte>>>
            presentation;
        std::optional<std::pair<std::string, std::string>> candidate_seed;
        std::optional<std::string> candidate_directory;
        std::optional<AndroidBootGenerationContract> android_boot;

        std::visit(
            [&](const auto &contract) {
                using Contract = std::decay_t<decltype(contract)>;
                if constexpr (std::is_same_v<Contract, DracutSystemdContract> ||
                              std::is_same_v<Contract, InitramfsToolsSystemdContract> ||
                              std::is_same_v<Contract, MkinitcpioSystemdContract>) {
                    boot_entry_path = contract.grub_script_path;
                    boot_entry_mode = 0755;
                    boot_entry_content = contract.grub_script;
                    boot_config_path = contract.grub_config_path;
                    boot_update = contract.update_grub;
                } else if constexpr (std::is_same_v<Contract, MkinitfsOpenRcContract>) {
                    boot_entry_path = contract.extlinux_fragment_path;
                    boot_entry_mode = 0644;
                    boot_entry_content = contract.extlinux_fragment;
                    boot_config_path = contract.extlinux_config_path;
                    boot_update = contract.update_extlinux;
                } else {
                    boot_entry_path = contract.known_good_entry_path;
                    boot_entry_mode = contract.known_good_entry_mode;
                    boot_entry_content = contract.known_good_entry;
                    presentation =
                        std::tuple{contract.active_loader_entry, contract.active_loader_entry_mode,
                                   contract.active_loader_entry_original, contract.active_loader_entry_activated};
                    candidate_seed = std::pair{contract.kernel_image, contract.candidate_kernel};
                    candidate_directory = contract.candidate_directory;
                    android_boot = contract.android_boot;
                }
                if constexpr (std::is_same_v<Contract, MkinitcpioSystemdContract>) {
                    configuration = std::tuple{contract.config_path, contract.config_mode, contract.config_original,
                                               contract.config_activated};
                } else if constexpr (std::is_same_v<Contract, MkinitfsOpenRcContract>) {
                    configuration = std::tuple{contract.mkinitfs_config_path, contract.mkinitfs_config_mode,
                                               contract.mkinitfs_config_original, contract.mkinitfs_config_activated};
                }
            },
            discovery.contract);

        const auto expected_bootart = read_running_elf();
        const auto expected_bootart_digest = sha256(expected_bootart);
        if (!std::ranges::any_of(plan.operations, [&](const PlanOperation &operation) {
                return operation.path == "/usr/bin/bootart" && operation.digest == expected_bootart_digest &&
                       operation.content == expected_bootart;
            })) {
            throw std::runtime_error("exact transaction ELF differs from the install plan");
        }

        if (const auto existing = read_manifest(root_, expected_owner_uid_)) {
            if (existing->plan_id != plan.identity() || existing->initramfs != plan.initramfs ||
                existing->real_root != plan.real_root) {
                throw std::runtime_error("an installation with a different plan already exists");
            }
            const auto owns = [&](std::string_view path) {
                return std::ranges::any_of(existing->entries, [&](const Entry &entry) { return entry.path == path; });
            };
            if (!owns(active_path) || !owns(known_good_path) || !owns(boot_entry_path)) {
                throw std::runtime_error("installed image contract differs from the running kernel contract");
            }
            if (!existing->image || existing->image->active_image != active_path ||
                existing->image->known_good_image != known_good_path ||
                existing->image->grub_script_path != boot_entry_path) {
                throw std::runtime_error("installed image record differs from the running kernel contract");
            }
            bool active_modified = false;
            for (const auto &entry : existing->entries) {
                try {
                    verify_expected_current(root_, entry, expected_owner_uid_);
                } catch (...) {
                    if (entry.path != active_path)
                        throw;
                    active_modified = true;
                }
            }
            if (existing->raw_boot) {
                const auto current = read_android_boot_partition(existing->raw_boot->partition, false);
                if (sha256(current) != existing->raw_boot->installed_digest)
                    throw std::runtime_error("managed Android boot partition changed");
                static_cast<void>(read_raw_backup(root_, *existing->raw_boot, expected_owner_uid_));
            }
            if (!active_modified)
                return ApplyOutcome::already_current;
            if (!android_boot || !existing->raw_boot || !candidate_seed || !candidate_directory || !presentation ||
                discovery.initramfs != AdapterId::mkinitfs_boot_deploy) {
                throw std::runtime_error("managed active initramfs changed outside the Android package refresh path");
            }
            const auto &android = *android_boot;
            const auto &old_raw = *existing->raw_boot;
            const auto &old_image = *existing->image;
            const auto &[loader_path, loader_mode, loader_original, loader_activated] = *presentation;
            static_cast<void>(loader_original);
            if (old_image.candidate_image != candidate_path || old_image.grub_config_path != loader_path ||
                old_raw.partition.label != android.partition.label ||
                old_raw.partition.canonical_path != android.partition.canonical_path ||
                old_raw.partition.device_number != android.partition.device_number ||
                old_raw.partition.bytes != android.partition.bytes ||
                android.partition.digest != old_raw.installed_digest) {
                throw std::runtime_error("Android refresh contract differs from the installed raw image identity");
            }
            std::uint16_t current_loader_mode{};
            const auto current_loader =
                read_regular(host_path(root_, loader_path), expected_owner_uid_, &current_loader_mode, max_state_bytes);
            if (current_loader_mode != loader_mode || current_loader != loader_activated)
                throw std::runtime_error("managed Android loader entry changed");
            std::uint16_t deviceinfo_mode{};
            const auto deviceinfo = read_regular(host_path(root_, android.deviceinfo_path), expected_owner_uid_,
                                                 &deviceinfo_mode, max_candidate_bytes);
            if (deviceinfo_mode != 0600 || deviceinfo != android.deviceinfo.no_flash_deviceinfo)
                throw std::runtime_error("managed Android no-flash guard changed");

            std::uint16_t active_mode{};
            const auto active =
                read_regular(host_path(root_, active_path), expected_owner_uid_, &active_mode, max_candidate_bytes);
            std::uint16_t kernel_mode{};
            const auto kernel = read_regular(host_path(root_, candidate_seed->first), expected_owner_uid_, &kernel_mode,
                                             max_candidate_bytes);
            std::uint16_t dtb_mode{};
            const auto dtb =
                read_regular(host_path(root_, android.dtb_path), expected_owner_uid_, &dtb_mode, max_candidate_bytes);
            if (active.empty() || (active_mode & 0022) != 0 || kernel.empty() || (kernel_mode & 0022) != 0 ||
                dtb.empty() || (dtb_mode & 0022) != 0 || dtb.size() != android.dtb_bytes ||
                sha256(dtb) != android.dtb_digest) {
                throw std::runtime_error("Android refresh image inputs are unsafe");
            }
            const auto raw_current = read_android_boot_partition(android.partition, false);
            if (sha256(raw_current) != old_raw.installed_digest)
                throw std::runtime_error("managed Android boot partition changed");

            std::vector<Entry> refresh_entries;
            auto capture = [&](std::string path, EntryKind kind) {
                Entry entry;
                entry.kind = kind;
                entry.path = std::move(path);
                entry.original = capture_preimage(root_, entry.path, expected_owner_uid_);
                refresh_entries.push_back(std::move(entry));
                return refresh_entries.size() - 1;
            };
            const auto active_index = capture(active_path, EntryKind::patched_file);
            const auto candidate_index = capture(candidate_path, EntryKind::transient_file);
            const auto seed_index = capture(candidate_seed->second, EntryKind::transient_file);
            const auto boot_image_index = capture(android.candidate_boot_image, EntryKind::transient_file);
            const auto manifest_index = capture(std::string(manifest_path), EntryKind::patched_file);
            for (const auto index : {candidate_index, seed_index, boot_image_index}) {
                if (refresh_entries[index].original.kind != PreimageKind::absent)
                    throw std::runtime_error("Android refresh candidate destination already exists");
            }

            const auto transaction = transaction_id();
            Journal journal{JournalKind::refresh,       "bootstrap", transaction,
                            std::move(refresh_entries), {},          std::nullopt};
            auto refresh_partition = android.partition;
            refresh_partition.digest = old_raw.installed_digest;
            journal.raw_boot = RawBootJournal{std::move(refresh_partition),
                                              old_raw.installed_digest,
                                              {},
                                              std::string(transactions_path) + "/" + transaction + "/raw-boot-preimage",
                                              Progress::planned};
            journal.created_dirs =
                required_missing_directories(root_, journal.entries, journal.transaction, expected_owner_uid_);
            write_journal(root_, journal);
            bool manifest_committed = false;
            try {
                create_directories(root_, journal.created_dirs, expected_owner_uid_);
                store_backups(root_, journal, expected_owner_uid_);
                store_raw_backup(root_, journal, raw_current, expected_owner_uid_);
                journal.phase = "ready";
                write_journal(root_, journal);

                auto &seed = journal.entries[seed_index];
                seed.progress = Progress::in_progress;
                write_journal(root_, journal);
                atomic_write(host_path(root_, seed.path), kernel, kernel_mode);
                seed.installed_mode = kernel_mode;
                seed.installed_digest = sha256(kernel);
                seed.progress = Progress::applied;
                write_journal(root_, journal);

                journal.entries[candidate_index].progress = Progress::in_progress;
                journal.entries[boot_image_index].progress = Progress::in_progress;
                write_journal(root_, journal);
                static_cast<void>(run_generator(generate));
                std::uint16_t candidate_mode{};
                const auto candidate = read_regular(host_path(root_, candidate_path), expected_owner_uid_,
                                                    &candidate_mode, max_candidate_bytes);
                std::uint16_t boot_image_mode{};
                const auto boot_image = read_regular(host_path(root_, android.candidate_boot_image),
                                                     expected_owner_uid_, &boot_image_mode, max_candidate_bytes);
                if (candidate.empty() || (candidate_mode & 0022) != 0 || boot_image.empty() ||
                    (boot_image_mode & 0022) != 0 || boot_image.size() > android.partition.bytes) {
                    throw std::runtime_error("Android refresh generated unsafe images");
                }
                journal.entries[candidate_index].installed_mode = candidate_mode;
                journal.entries[candidate_index].installed_digest = sha256(candidate);
                journal.entries[candidate_index].progress = Progress::applied;
                journal.entries[boot_image_index].installed_mode = boot_image_mode;
                journal.entries[boot_image_index].installed_digest = sha256(boot_image);
                journal.entries[boot_image_index].progress = Progress::applied;
                write_journal(root_, journal);
                unlink_file_or_link(host_path(root_, candidate_seed->second));
                const std::array refresh_keep{candidate_path, android.candidate_boot_image};
                prune_boot_deploy_candidate(root_, *candidate_directory, refresh_keep, expected_owner_uid_);

                const auto archive = std::visit(
                    [&](const auto &contract) -> std::vector<std::byte> {
                        using Contract = std::decay_t<decltype(contract)>;
                        if constexpr (std::is_same_v<Contract, MkinitfsBootDeployContract>)
                            return decompress_mkinitfs_boot_deploy_archive(candidate, contract.expected_compression);
                        throw std::runtime_error("Android refresh has a non-boot-deploy contract");
                    },
                    discovery.contract);
                const auto inspection = inspect_mkinitfs_boot_deploy_archive(archive, expected_bootart);
                auto new_image = std::visit(
                    [&](const auto &contract) -> DracutSystemdImageRecord {
                        using Contract = std::decay_t<decltype(contract)>;
                        if constexpr (std::is_same_v<Contract, MkinitfsBootDeployContract>)
                            return verified_mkinitfs_boot_deploy_image_record(contract, candidate, inspection,
                                                                              expected_bootart);
                        throw std::runtime_error("Android refresh has a non-boot-deploy contract");
                    },
                    discovery.contract);
                static_cast<void>(inspect_android_boot_image_v2(boot_image, kernel, candidate, dtb));

                auto &active_entry = journal.entries[active_index];
                active_entry.progress = Progress::in_progress;
                write_journal(root_, journal);
                const auto candidate_host = host_path(root_, candidate_path);
                const auto active_host = host_path(root_, active_path);
                if (rename(candidate_host.c_str(), active_host.c_str()) != 0)
                    throw system_error("activate refreshed initramfs", active_host);
                fsync_directory(active_host.parent_path());
                active_entry.installed_mode = candidate_mode;
                active_entry.installed_digest = new_image.active_digest;
                active_entry.progress = Progress::applied;
                write_journal(root_, journal);

                auto &raw = *journal.raw_boot;
                raw.progress = Progress::in_progress;
                write_journal(root_, journal);
                raw.installed_digest =
                    activate_android_boot_partition_durable(android.partition, raw_current, boot_image);
                raw.progress = Progress::applied;
                write_journal(root_, journal);

                new_image.known_good_image = old_image.known_good_image;
                new_image.known_good_digest = old_image.known_good_digest;
                new_image.grub_script_path = old_image.grub_script_path;
                new_image.grub_script_digest = old_image.grub_script_digest;
                new_image.grub_config_path = loader_path;
                validate_mkinitfs_boot_deploy_image_record(new_image);
                auto refreshed = *existing;
                bool updated_active = false;
                for (auto &entry : refreshed.entries) {
                    if (entry.path != active_path)
                        continue;
                    entry.installed_mode = candidate_mode;
                    entry.installed_digest = new_image.active_digest;
                    updated_active = true;
                }
                if (!updated_active)
                    throw std::runtime_error("Android refresh manifest has no active image entry");
                refreshed.transaction = transaction;
                refreshed.image = std::move(new_image);
                refreshed.raw_boot->installed_digest = raw.installed_digest;
                unlink_file_or_link(host_path(root_, android.candidate_boot_image));
                const auto candidate_root = host_path(root_, *candidate_directory);
                if (rmdir(candidate_root.c_str()) != 0 && errno != ENOENT)
                    throw system_error("remove Android refresh candidate directory", candidate_root);

                auto &manifest_entry = journal.entries[manifest_index];
                manifest_entry.progress = Progress::in_progress;
                write_journal(root_, journal);
                const auto document = serialize_manifest(refreshed);
                atomic_write_text(host_path(root_, manifest_path), document, 0600);
                manifest_entry.installed_mode = 0600;
                manifest_entry.installed_digest = sha256(std::as_bytes(std::span(document)));
                manifest_entry.progress = Progress::applied;
                write_journal(root_, journal);
                manifest_committed = true;
                journal.phase = "cleanup";
                write_journal(root_, journal);
                cleanup_transaction_backups(root_, journal);
                static_cast<void>(remove_empty_directories(root_, journal.created_dirs));
                unlink_file_or_link(host_path(root_, journal_path));
                return ApplyOutcome::refreshed;
            } catch (...) {
                const auto failure = std::current_exception();
                if (!manifest_committed) {
                    try {
                        rollback(root_, journal, expected_owner_uid_);
                    } catch (...) {
                    }
                }
                std::rethrow_exception(failure);
            }
        }

        auto entries = build_actions(root_, plan, expected_owner_uid_);
        auto append_file = [&](std::string path, std::uint16_t mode, std::vector<std::byte> content, EntryKind kind,
                               bool must_be_absent) {
            if (std::ranges::any_of(entries, [&](const Entry &entry) { return entry.path == path; })) {
                throw std::runtime_error("exact transaction contains a duplicate path: " + path);
            }
            Entry entry;
            entry.kind = kind;
            entry.path = std::move(path);
            entry.installed_mode = mode;
            entry.installed_digest = content.empty() ? std::string{} : sha256(content);
            entry.content = std::move(content);
            entry.original = capture_preimage(root_, entry.path, expected_owner_uid_);
            if (must_be_absent && entry.original.kind != PreimageKind::absent) {
                throw std::runtime_error("unowned exact installer destination already exists: " + entry.path);
            }
            entries.push_back(std::move(entry));
            return entries.size() - 1;
        };

        if (configuration) {
            auto &[path, mode, original, activated] = *configuration;
            const auto current = read_regular(host_path(root_, path), expected_owner_uid_);
            std::uint16_t current_mode{};
            static_cast<void>(read_regular(host_path(root_, path), expected_owner_uid_, &current_mode));
            if (current != original || current_mode != mode) {
                throw std::runtime_error("generator configuration changed after discovery");
            }
            static_cast<void>(append_file(path, mode, activated, EntryKind::patched_file, false));
        }
        if (presentation) {
            auto &[path, mode, original, activated] = *presentation;
            std::uint16_t current_mode{};
            const auto current = read_regular(host_path(root_, path), expected_owner_uid_, &current_mode);
            if (current != original || current_mode != mode) {
                throw std::runtime_error("active loader entry changed after discovery");
            }
            static_cast<void>(append_file(path, mode, activated, EntryKind::patched_file, false));
        }
        std::vector<std::byte> raw_boot_preimage;
        std::vector<std::byte> android_dtb;
        if (android_boot) {
            std::uint16_t dtb_mode{};
            android_dtb = read_regular(host_path(root_, android_boot->dtb_path), expected_owner_uid_, &dtb_mode,
                                       max_candidate_bytes);
            if (android_dtb.empty() || android_dtb.size() != android_boot->dtb_bytes ||
                sha256(android_dtb) != android_boot->dtb_digest || (dtb_mode & 0022) != 0) {
                throw std::runtime_error("Android DTB changed after discovery");
            }
            raw_boot_preimage = read_android_boot_partition(android_boot->partition, true);
            static_cast<void>(append_file(android_boot->deviceinfo_path, 0600,
                                          android_boot->deviceinfo.no_flash_deviceinfo, EntryKind::patched_file,
                                          false));
        }
        const auto deterministic_count = entries.size();

        std::uint16_t active_mode{};
        const auto active_original =
            read_regular(host_path(root_, active_path), expected_owner_uid_, &active_mode, max_candidate_bytes);
        if (active_original.empty() || sha256(active_original) != known_good_digest || (active_mode & 0022) != 0) {
            throw std::runtime_error("active initramfs changed after discovery");
        }
        const auto known_good_index = append_file(known_good_path, active_mode, active_original, EntryKind::file, true);
        const auto boot_entry_index =
            append_file(boot_entry_path, boot_entry_mode, boot_entry_content, EntryKind::file, true);

        std::optional<std::size_t> seed_index;
        std::vector<std::byte> seed_bytes;
        std::uint16_t seed_mode{};
        if (candidate_seed) {
            seed_bytes = read_regular(host_path(root_, candidate_seed->first), expected_owner_uid_, &seed_mode,
                                      max_candidate_bytes);
            if (seed_bytes.empty() || (seed_mode & 0022) != 0) {
                throw std::runtime_error("candidate kernel seed is unsafe");
            }
            seed_index = append_file(candidate_seed->second, seed_mode, seed_bytes, EntryKind::transient_file, true);
        }
        const auto candidate_index = append_file(candidate_path, 0600, {}, EntryKind::transient_file, true);
        const auto active_index = append_file(active_path, active_mode, {}, EntryKind::patched_file, false);
        std::optional<std::size_t> boot_config_index;
        if (boot_config_path) {
            std::uint16_t mode{};
            const auto original =
                read_regular(host_path(root_, *boot_config_path), expected_owner_uid_, &mode, 16 * 1024 * 1024);
            if (original.empty() || (mode & 0022) != 0) {
                throw std::runtime_error("boot-loader configuration is unsafe");
            }
            boot_config_index = append_file(*boot_config_path, mode, {}, EntryKind::patched_file, false);
        }

        const auto transaction = transaction_id();
        Journal journal{JournalKind::install, "bootstrap", transaction, std::move(entries), {}, std::nullopt};
        if (android_boot) {
            journal.raw_boot = RawBootJournal{android_boot->partition,
                                              android_boot->partition.digest,
                                              {},
                                              std::string(transactions_path) + "/" + transaction + "/raw-boot-preimage",
                                              Progress::planned};
        }
        journal.created_dirs =
            required_missing_directories(root_, journal.entries, journal.transaction, expected_owner_uid_);
        write_journal(root_, journal);
        bool manifest_committed = false;
        auto mark_and_apply = [&](std::size_t index) {
            journal.entries[index].progress = Progress::in_progress;
            write_journal(root_, journal);
            apply_entry(root_, journal.entries[index], expected_owner_uid_);
            journal.entries[index].progress = Progress::applied;
            write_journal(root_, journal);
        };
        try {
            create_directories(root_, journal.created_dirs, expected_owner_uid_);
            store_backups(root_, journal, expected_owner_uid_);
            if (android_boot)
                store_raw_backup(root_, journal, raw_boot_preimage, expected_owner_uid_);
            journal.phase = "ready";
            write_journal(root_, journal);
            for (std::size_t index = 0; index < deterministic_count; ++index)
                mark_and_apply(index);
            mark_and_apply(known_good_index);
            mark_and_apply(boot_entry_index);
            if (seed_index)
                mark_and_apply(*seed_index);

            journal.entries[candidate_index].progress = Progress::in_progress;
            write_journal(root_, journal);
            static_cast<void>(run_generator(generate));
            std::uint16_t candidate_mode{};
            const auto candidate = read_regular(host_path(root_, candidate_path), expected_owner_uid_, &candidate_mode,
                                                max_candidate_bytes);
            if (candidate.empty() || (candidate_mode & 0022) != 0) {
                throw std::runtime_error("generated initramfs candidate is unsafe");
            }
            journal.entries[candidate_index].installed_mode = candidate_mode;
            journal.entries[candidate_index].installed_digest = sha256(candidate);
            journal.entries[candidate_index].progress = Progress::applied;
            write_journal(root_, journal);

            std::vector<std::byte> candidate_boot_image;
            if (android_boot) {
                std::uint16_t boot_image_mode{};
                candidate_boot_image = read_regular(host_path(root_, android_boot->candidate_boot_image),
                                                    expected_owner_uid_, &boot_image_mode, max_candidate_bytes);
                if (candidate_boot_image.empty() || (boot_image_mode & 0022) != 0 ||
                    candidate_boot_image.size() > android_boot->partition.bytes) {
                    throw std::runtime_error("generated Android boot image is unsafe");
                }
                std::uint16_t deviceinfo_mode{};
                const auto deviceinfo = read_regular(host_path(root_, android_boot->deviceinfo_path),
                                                     expected_owner_uid_, &deviceinfo_mode, max_candidate_bytes);
                if (deviceinfo_mode != 0600 || deviceinfo != android_boot->deviceinfo.no_flash_deviceinfo)
                    throw std::runtime_error("Android no-flash guard changed during generation");
            }
            if (candidate_directory) {
                std::vector<std::string> keep{candidate_path};
                if (android_boot)
                    keep.push_back(android_boot->candidate_boot_image);
                prune_boot_deploy_candidate(root_, *candidate_directory, keep, expected_owner_uid_);
            }

            if (seed_index) {
                unlink_file_or_link(host_path(root_, candidate_seed->second));
            }

            ArchiveInspection inspection{};
            bool direct = false;
            DracutSystemdImageRecord image_record{};
            std::visit(
                [&](const auto &contract) {
                    using Contract = std::decay_t<decltype(contract)>;
                    if constexpr (std::is_same_v<Contract, MkinitfsOpenRcContract>) {
                        inspection = inspect_mkinitfs_openrc_archive(candidate, expected_bootart);
                        direct = true;
                    } else if constexpr (std::is_same_v<Contract, MkinitfsBootDeployContract>) {
                        const auto archive =
                            decompress_mkinitfs_boot_deploy_archive(candidate, contract.expected_compression);
                        inspection = inspect_mkinitfs_boot_deploy_archive(archive, expected_bootart);
                        direct = true;
                    }
                },
                discovery.contract);
            if (!direct) {
                const auto inspection_guest =
                    std::string(transactions_path) + "/" + transaction + "/unpacked-candidate";
                const auto inspection_host = host_path(root_, inspection_guest);
                if (mkdir(inspection_host.c_str(), 0700) != 0) {
                    throw system_error("create inspection directory", inspection_host);
                }
                GeneratorRequest unpack{};
                std::visit(
                    [&](const auto &contract) {
                        using Contract = std::decay_t<decltype(contract)>;
                        if constexpr (std::is_same_v<Contract, DracutSystemdContract>) {
                            unpack = dracut_systemd_unpack_request(contract, transaction);
                        } else if constexpr (std::is_same_v<Contract, InitramfsToolsSystemdContract>) {
                            unpack = initramfs_tools_systemd_unpack_request(contract, transaction);
                        } else if constexpr (std::is_same_v<Contract, MkinitcpioSystemdContract>) {
                            unpack = mkinitcpio_unpack_request(contract, transaction);
                        }
                    },
                    discovery.contract);
                static_cast<void>(run_generator(unpack));
                std::visit(
                    [&](const auto &contract) {
                        using Contract = std::decay_t<decltype(contract)>;
                        if constexpr (std::is_same_v<Contract, DracutSystemdContract>) {
                            inspection = inspect_dracut_inventory(
                                collect_unpacked_archive_inventory(inspection_host.string(), expected_owner_uid_),
                                expected_bootart);
                        } else if constexpr (std::is_same_v<Contract, InitramfsToolsSystemdContract>) {
                            inspection =
                                inspect_initramfs_tools_inventory(collect_unpacked_initramfs_tools_inventory(
                                                                      inspection_host.string(), expected_owner_uid_),
                                                                  expected_bootart);
                        } else if constexpr (std::is_same_v<Contract, MkinitcpioSystemdContract>) {
                            inspection = inspect_mkinitcpio_inventory(
                                collect_unpacked_archive_inventory(inspection_host.string(), expected_owner_uid_),
                                expected_bootart);
                        }
                    },
                    discovery.contract);
                std::error_code cleanup_error;
                std::filesystem::remove_all(inspection_host, cleanup_error);
                if (cleanup_error)
                    throw std::runtime_error("remove inspection tree: " + cleanup_error.message());
            }

            std::visit(
                [&](const auto &contract) {
                    using Contract = std::decay_t<decltype(contract)>;
                    if constexpr (std::is_same_v<Contract, DracutSystemdContract>) {
                        image_record =
                            verified_dracut_systemd_image_record(contract, candidate, inspection, expected_bootart);
                    } else if constexpr (std::is_same_v<Contract, InitramfsToolsSystemdContract>) {
                        image_record = verified_initramfs_tools_systemd_image_record(contract, candidate, inspection,
                                                                                     expected_bootart);
                    } else if constexpr (std::is_same_v<Contract, MkinitcpioSystemdContract>) {
                        image_record =
                            verified_mkinitcpio_systemd_image_record(contract, candidate, inspection, expected_bootart);
                    } else if constexpr (std::is_same_v<Contract, MkinitfsOpenRcContract>) {
                        image_record =
                            verified_mkinitfs_openrc_image_record(contract, candidate, inspection, expected_bootart);
                    } else {
                        image_record = verified_mkinitfs_boot_deploy_image_record(contract, candidate, inspection,
                                                                                  expected_bootart);
                    }
                },
                discovery.contract);
            if (android_boot) {
                if (seed_bytes.empty() || android_dtb.empty())
                    throw std::runtime_error("Android boot image inputs were not captured");
                static_cast<void>(
                    inspect_android_boot_image_v2(candidate_boot_image, seed_bytes, candidate, android_dtb));
            }

            if (boot_update && boot_config_index) {
                auto &entry = journal.entries[*boot_config_index];
                entry.progress = Progress::in_progress;
                write_journal(root_, journal);
                static_cast<void>(run_generator(*boot_update));
                std::uint16_t mode{};
                const auto updated =
                    read_regular(host_path(root_, *boot_config_path), expected_owner_uid_, &mode, 16 * 1024 * 1024);
                const std::string updated_text(reinterpret_cast<const char *>(updated.data()), updated.size());
                if (updated.empty() || !updated_text.contains("bootart-known-good")) {
                    throw std::runtime_error("updated boot-loader configuration omits the known-good entry");
                }
                entry.installed_mode = mode;
                entry.installed_digest = sha256(updated);
                entry.progress = Progress::applied;
                write_journal(root_, journal);
            } else if (boot_update || boot_config_index) {
                throw std::runtime_error("boot-loader update contract is incomplete");
            }

            auto &active_entry = journal.entries[active_index];
            active_entry.progress = Progress::in_progress;
            write_journal(root_, journal);
            const auto candidate_host = host_path(root_, candidate_path);
            const auto active_host = host_path(root_, active_path);
            if (rename(candidate_host.c_str(), active_host.c_str()) != 0) {
                throw system_error("activate candidate initramfs", active_host);
            }
            fsync_directory(active_host.parent_path());
            active_entry.installed_mode = candidate_mode;
            active_entry.installed_digest = sha256(candidate);
            active_entry.progress = Progress::applied;
            write_journal(root_, journal);
            std::optional<RawBootManifest> raw_manifest;
            if (android_boot) {
                auto &raw = *journal.raw_boot;
                raw.progress = Progress::in_progress;
                write_journal(root_, journal);
                raw.installed_digest = activate_android_boot_partition_durable(android_boot->partition,
                                                                               raw_boot_preimage, candidate_boot_image);
                raw.progress = Progress::applied;
                write_journal(root_, journal);
                raw_manifest = RawBootManifest{android_boot->partition, android_boot->partition.digest,
                                               raw.installed_digest, raw.backup};
                unlink_file_or_link(host_path(root_, android_boot->candidate_boot_image));
            }
            if (candidate_directory) {
                const auto directory = host_path(root_, *candidate_directory);
                if (rmdir(directory.c_str()) != 0 && errno != ENOENT) {
                    throw system_error("remove candidate directory", directory);
                }
            }

            std::vector<Entry> manifest_entries;
            manifest_entries.reserve(journal.entries.size());
            for (auto entry : journal.entries) {
                if (entry.kind == EntryKind::transient_file)
                    continue;
                entry.progress = Progress::planned;
                entry.content.clear();
                entry.original.captured.clear();
                manifest_entries.push_back(std::move(entry));
            }
            std::ranges::sort(manifest_entries, {}, &Entry::path);
            Manifest manifest{journal.transaction,
                              plan.identity(),
                              plan.initramfs,
                              plan.real_root,
                              image_record,
                              std::move(raw_manifest),
                              std::move(manifest_entries),
                              journal.created_dirs};
            write_manifest(root_, manifest);
            manifest_committed = true;
            journal.phase = "cleanup";
            write_journal(root_, journal);
            unlink_file_or_link(host_path(root_, journal_path));
            return ApplyOutcome::installed;
        } catch (...) {
            const auto failure = std::current_exception();
            if (!manifest_committed) {
                try {
                    rollback(root_, journal, expected_owner_uid_);
                } catch (...) {
                }
            }
            std::rethrow_exception(failure);
        }
    }

    RecoveryOutcome Installer::recover() {
        if (!mutation_unlocked_)
            throw std::runtime_error("installer mutation is locked");
        auto lock = lock_root(root_, expected_owner_uid_);
        auto journal = read_journal(root_, expected_owner_uid_);
        if (!journal)
            return RecoveryOutcome::no_transaction;
        const auto committed_manifest = read_manifest(root_, expected_owner_uid_);
        const bool committed_manifest_transaction = journal->kind != JournalKind::uninstall && committed_manifest &&
                                                    committed_manifest->transaction == journal->transaction;
        if (journal->phase == "cleanup" || committed_manifest_transaction) {
            if (journal->kind == JournalKind::install || journal->kind == JournalKind::refresh) {
                if (!committed_manifest || committed_manifest->transaction != journal->transaction) {
                    throw std::runtime_error("committed installer journal has no matching manifest");
                }
                for (const auto &entry : committed_manifest->entries) {
                    verify_expected_current(root_, entry, expected_owner_uid_);
                }
                if (committed_manifest->raw_boot) {
                    const auto &raw = *committed_manifest->raw_boot;
                    const auto current = read_android_boot_partition(raw.partition, false);
                    if (sha256(current) != raw.installed_digest)
                        throw std::runtime_error("committed Android boot partition differs from the manifest");
                    static_cast<void>(read_raw_backup(root_, raw, expected_owner_uid_));
                }
                if (journal->kind == JournalKind::refresh) {
                    cleanup_transaction_backups(root_, *journal);
                    static_cast<void>(remove_empty_directories(root_, journal->created_dirs));
                }
            } else {
                if (journal->raw_boot && journal->raw_boot->progress == Progress::applied) {
                    const auto current = read_android_boot_partition(journal->raw_boot->partition, false);
                    if (journal->raw_boot->installed_digest.empty() ||
                        sha256(current) != journal->raw_boot->installed_digest) {
                        throw std::runtime_error("committed raw boot uninstall differs from its journal");
                    }
                }
                if (const auto manifest = read_manifest(root_, expected_owner_uid_)) {
                    cleanup_manifest_backups(root_, *manifest);
                    unlink_file_or_link(host_path(root_, manifest_path));
                    static_cast<void>(remove_empty_directories(root_, manifest->created_dirs));
                }
                cleanup_transaction_backups(root_, *journal);
                static_cast<void>(remove_empty_directories(root_, journal->created_dirs));
            }
            unlink_file_or_link(host_path(root_, journal_path));
            return RecoveryOutcome::completed_commit_cleaned;
        }
        rollback(root_, *journal, expected_owner_uid_);
        return RecoveryOutcome::rolled_back;
    }

    UninstallReport Installer::uninstall(const ExactInstallDiscovery *discovery) {
        if (!mutation_unlocked_)
            throw std::runtime_error("installer mutation is locked");
        auto lock = lock_root(root_, expected_owner_uid_);
        if (read_journal(root_, expected_owner_uid_))
            throw std::runtime_error("explicit recovery is required");
        const auto manifest = read_manifest(root_, expected_owner_uid_);
        if (!manifest)
            return {};
        for (const auto &entry : manifest->entries)
            verify_expected_current(root_, entry, expected_owner_uid_);

        const DracutSystemdContract *dracut = nullptr;
        const MkinitfsBootDeployContract *boot_deploy = nullptr;
        if (discovery) {
            if (discovery->initramfs != manifest->initramfs || discovery->real_root != manifest->real_root)
                throw std::runtime_error("uninstall discovery differs from the installed adapter pair");
            dracut = std::get_if<DracutSystemdContract>(&discovery->contract);
            boot_deploy = std::get_if<MkinitfsBootDeployContract>(&discovery->contract);
        }
        const bool clean_dracut = dracut != nullptr && manifest->initramfs == AdapterId::dracut_systemd;
        const bool clean_boot_deploy = boot_deploy != nullptr && manifest->initramfs == AdapterId::mkinitfs_boot_deploy;
        if ((clean_dracut || clean_boot_deploy) && !manifest->image)
            throw std::runtime_error("exact uninstall requires the installed image record");
        if (manifest->raw_boot && (!clean_boot_deploy || !boot_deploy->android_boot))
            throw std::runtime_error("Android uninstall requires the exact current-kernel boot-deploy contract");
        if (clean_dracut && (manifest->image->active_image != dracut->active_image ||
                             manifest->image->candidate_image != dracut->candidate_image)) {
            throw std::runtime_error("dracut uninstall contract differs from the installed image");
        }
        if (clean_boot_deploy && (manifest->image->active_image != boot_deploy->active_image ||
                                  manifest->image->candidate_image != boot_deploy->candidate_image ||
                                  manifest->image->grub_config_path != boot_deploy->active_loader_entry)) {
            throw std::runtime_error("boot-deploy uninstall contract differs from the installed image");
        }

        std::vector<std::byte> raw_installed;
        if (manifest->raw_boot) {
            const auto &android = *boot_deploy->android_boot;
            const auto &raw = *manifest->raw_boot;
            if (raw.partition.label != android.partition.label ||
                raw.partition.canonical_path != android.partition.canonical_path ||
                raw.partition.device_number != android.partition.device_number ||
                raw.partition.bytes != android.partition.bytes || raw.installed_digest != android.partition.digest) {
                throw std::runtime_error("Android uninstall contract differs from the installed partition identity");
            }
            raw_installed = read_android_boot_partition(raw.partition, false);
            if (sha256(raw_installed) != raw.installed_digest)
                throw std::runtime_error("managed Android boot partition changed");
            static_cast<void>(read_raw_backup(root_, raw, expected_owner_uid_));
        }

        std::vector<Entry> uninstall_entries = manifest->entries;
        const auto manifest_count = uninstall_entries.size();
        auto append_transient = [&](std::string path) {
            if (std::ranges::any_of(uninstall_entries, [&](const Entry &entry) { return entry.path == path; }))
                throw std::runtime_error("uninstall candidate duplicates a managed path");
            Entry entry;
            entry.kind = EntryKind::transient_file;
            entry.path = std::move(path);
            entry.original = capture_preimage(root_, entry.path, expected_owner_uid_);
            if (entry.original.kind != PreimageKind::absent)
                throw std::runtime_error("uninstall candidate destination already exists: " + entry.path);
            uninstall_entries.push_back(std::move(entry));
            return uninstall_entries.size() - 1;
        };
        std::optional<std::size_t> candidate_index;
        std::optional<std::size_t> seed_index;
        std::optional<std::size_t> boot_image_index;
        if (clean_dracut)
            candidate_index = append_transient(manifest->image->candidate_image);
        if (clean_boot_deploy) {
            candidate_index = append_transient(boot_deploy->candidate_image);
            seed_index = append_transient(boot_deploy->candidate_kernel);
            if (boot_deploy->android_boot)
                boot_image_index = append_transient(boot_deploy->android_boot->candidate_boot_image);
        }

        Journal journal{JournalKind::uninstall,       "bootstrap", transaction_id(),
                        std::move(uninstall_entries), {},          std::nullopt};
        for (auto &entry : journal.entries) {
            entry.original = capture_preimage(root_, entry.path, expected_owner_uid_);
            entry.progress = Progress::planned;
        }
        journal.created_dirs =
            required_missing_directories(root_, journal.entries, journal.transaction, expected_owner_uid_);
        if (manifest->raw_boot) {
            auto partition = manifest->raw_boot->partition;
            partition.digest = manifest->raw_boot->installed_digest;
            journal.raw_boot =
                RawBootJournal{std::move(partition),
                               manifest->raw_boot->installed_digest,
                               {},
                               std::string(transactions_path) + "/" + journal.transaction + "/raw-boot-preimage",
                               Progress::planned};
        }
        write_journal(root_, journal);
        bool uninstall_committed = false;
        try {
            create_directories(root_, journal.created_dirs, expected_owner_uid_);
            store_backups(root_, journal, expected_owner_uid_);
            if (manifest->raw_boot)
                store_raw_backup(root_, journal, raw_installed, expected_owner_uid_);
            journal.phase = "ready";
            write_journal(root_, journal);

            auto manifest_index = [&](std::string_view path) {
                for (std::size_t index = 0; index < manifest_count; ++index) {
                    if (manifest->entries[index].path == path)
                        return index;
                }
                throw std::runtime_error("uninstall manifest lacks a required managed path: " + std::string(path));
            };
            auto restore_index = [&](std::size_t index,
                                     std::optional<std::pair<std::span<const std::byte>, std::uint16_t>> replacement =
                                         std::nullopt) {
                auto &progress = journal.entries[index];
                if (progress.progress == Progress::applied)
                    return;
                progress.progress = Progress::in_progress;
                write_journal(root_, journal);
                if (replacement)
                    atomic_write(host_path(root_, progress.path), replacement->first, replacement->second);
                else
                    restore_preimage(root_, manifest->entries[index], expected_owner_uid_);
                progress.progress = Progress::applied;
                write_journal(root_, journal);
            };

            std::vector<std::byte> clean_candidate;
            std::uint16_t clean_candidate_mode{};
            std::vector<std::byte> clean_boot_image;
            std::uint16_t clean_boot_image_mode{};
            std::vector<std::byte> clean_loader;
            std::uint16_t clean_loader_mode{};

            if (clean_dracut) {
                auto &candidate_entry = journal.entries[*candidate_index];
                candidate_entry.progress = Progress::in_progress;
                write_journal(root_, journal);
                static_cast<void>(run_generator(dracut_systemd_bootart_free_generate_request(*manifest->image)));
                clean_candidate = read_regular(host_path(root_, manifest->image->candidate_image), expected_owner_uid_,
                                               &clean_candidate_mode, max_candidate_bytes);
                if (clean_candidate.empty() || (clean_candidate_mode & 0022) != 0)
                    throw std::runtime_error("Bootart-free dracut candidate is unsafe");
                candidate_entry.installed_mode = clean_candidate_mode;
                candidate_entry.installed_digest = sha256(clean_candidate);
                candidate_entry.progress = Progress::applied;
                write_journal(root_, journal);
                const auto inspection_guest =
                    std::string(transactions_path) + "/" + journal.transaction + "/unpacked-candidate";
                const auto inspection_host = host_path(root_, inspection_guest);
                if (mkdir(inspection_host.c_str(), 0700) != 0)
                    throw system_error("create uninstall inspection directory", inspection_host);
                static_cast<void>(
                    run_generator(dracut_systemd_bootart_free_unpack_request(*manifest->image, journal.transaction)));
                static_cast<void>(inspect_bootart_free_dracut_inventory(
                    collect_unpacked_archive_inventory(inspection_host.string(), expected_owner_uid_)));
                std::error_code cleanup_error;
                std::filesystem::remove_all(inspection_host, cleanup_error);
                if (cleanup_error)
                    throw std::runtime_error("remove uninstall inspection tree: " + cleanup_error.message());
            }

            if (clean_boot_deploy) {
                constexpr std::array<std::string_view, 5> clean_inputs{
                    "/etc/mkinitfs/files-extra/bootart", "/etc/mkinitfs/hooks-extra/50-bootart-start.sh",
                    "/etc/mkinitfs/hooks-cleanup/90-bootart-handoff.sh", "/usr/share/initramfs/init_functions_2nd.sh",
                    "/etc/kernel-cmdline.d/90-bootart.conf"};
                for (const auto path : clean_inputs)
                    restore_index(manifest_index(path));

                std::uint16_t kernel_mode{};
                const auto kernel = read_regular(host_path(root_, boot_deploy->kernel_image), expected_owner_uid_,
                                                 &kernel_mode, max_candidate_bytes);
                if (kernel.empty() || (kernel_mode & 0022) != 0)
                    throw std::runtime_error("boot-deploy uninstall kernel is unsafe");
                auto &seed_entry = journal.entries[*seed_index];
                seed_entry.progress = Progress::in_progress;
                write_journal(root_, journal);
                atomic_write(host_path(root_, seed_entry.path), kernel, kernel_mode);
                seed_entry.installed_mode = kernel_mode;
                seed_entry.installed_digest = sha256(kernel);
                seed_entry.progress = Progress::applied;
                write_journal(root_, journal);

                auto &candidate_entry = journal.entries[*candidate_index];
                candidate_entry.progress = Progress::in_progress;
                if (boot_image_index)
                    journal.entries[*boot_image_index].progress = Progress::in_progress;
                write_journal(root_, journal);
                static_cast<void>(run_generator(boot_deploy->generate));
                clean_candidate = read_regular(host_path(root_, boot_deploy->candidate_image), expected_owner_uid_,
                                               &clean_candidate_mode, max_candidate_bytes);
                const auto loader_name = std::filesystem::path(boot_deploy->active_loader_entry).filename().string();
                const auto candidate_loader = boot_deploy->candidate_directory + "/loader/entries/" + loader_name;
                clean_loader = read_regular(host_path(root_, candidate_loader), expected_owner_uid_, &clean_loader_mode,
                                            max_state_bytes);
                if (clean_candidate.empty() || (clean_candidate_mode & 0022) != 0 || clean_loader.empty() ||
                    clean_loader_mode != boot_deploy->active_loader_entry_mode) {
                    throw std::runtime_error("Bootart-free boot-deploy candidate is unsafe");
                }
                candidate_entry.installed_mode = clean_candidate_mode;
                candidate_entry.installed_digest = sha256(clean_candidate);
                candidate_entry.progress = Progress::applied;
                if (boot_image_index) {
                    clean_boot_image = read_regular(host_path(root_, boot_deploy->android_boot->candidate_boot_image),
                                                    expected_owner_uid_, &clean_boot_image_mode, max_candidate_bytes);
                    if (clean_boot_image.empty() || (clean_boot_image_mode & 0022) != 0 ||
                        clean_boot_image.size() > boot_deploy->android_boot->partition.bytes) {
                        throw std::runtime_error("Bootart-free Android boot image is unsafe");
                    }
                    auto &entry = journal.entries[*boot_image_index];
                    entry.installed_mode = clean_boot_image_mode;
                    entry.installed_digest = sha256(clean_boot_image);
                    entry.progress = Progress::applied;
                }
                write_journal(root_, journal);
                unlink_file_or_link(host_path(root_, boot_deploy->candidate_kernel));
                std::vector<std::string> keep{boot_deploy->candidate_image};
                if (boot_deploy->android_boot)
                    keep.push_back(boot_deploy->android_boot->candidate_boot_image);
                prune_boot_deploy_candidate(root_, boot_deploy->candidate_directory, keep, expected_owner_uid_);

                const auto archive =
                    decompress_mkinitfs_boot_deploy_archive(clean_candidate, boot_deploy->expected_compression);
                static_cast<void>(inspect_bootart_free_mkinitfs_boot_deploy_archive(archive));
                const std::string loader_text(reinterpret_cast<const char *>(clean_loader.data()), clean_loader.size());
                const auto [loader_kernel, loader_options] = parse_mkinitfs_boot_deploy_loader_entry(loader_text);
                if (loader_kernel != boot_deploy->kernel_image)
                    throw std::runtime_error("Bootart-free loader entry selects a different kernel");
                std::istringstream options(loader_options);
                std::string option;
                while (options >> option) {
                    if (option == "bootart=0" || option == "rd.bootart=0" || option == "bootart=1" ||
                        option == "rd.bootart=1" || option == "-splash") {
                        throw std::runtime_error("Bootart-free loader entry retains a Bootart override");
                    }
                }
                if (boot_deploy->android_boot) {
                    std::uint16_t dtb_mode{};
                    const auto dtb = read_regular(host_path(root_, boot_deploy->android_boot->dtb_path),
                                                  expected_owner_uid_, &dtb_mode, max_candidate_bytes);
                    if (dtb.empty() || (dtb_mode & 0022) != 0 || dtb.size() != boot_deploy->android_boot->dtb_bytes ||
                        sha256(dtb) != boot_deploy->android_boot->dtb_digest) {
                        throw std::runtime_error("boot-deploy uninstall DTB changed");
                    }
                    static_cast<void>(inspect_android_boot_image_v2(clean_boot_image, kernel, clean_candidate, dtb));
                }
            }

            if (clean_dracut || clean_boot_deploy) {
                const auto active_path = manifest->image->active_image;
                const auto active_index = manifest_index(active_path);
                auto &active_entry = journal.entries[active_index];
                active_entry.progress = Progress::in_progress;
                write_journal(root_, journal);
                const auto candidate_host = host_path(root_, manifest->image->candidate_image);
                const auto active_host = host_path(root_, active_path);
                if (rename(candidate_host.c_str(), active_host.c_str()) != 0)
                    throw system_error("activate Bootart-free initramfs", active_host);
                fsync_directory(active_host.parent_path());
                active_entry.progress = Progress::applied;
                write_journal(root_, journal);
            }

            if (manifest->raw_boot) {
                auto &raw = *journal.raw_boot;
                raw.progress = Progress::in_progress;
                write_journal(root_, journal);
                raw.installed_digest = activate_android_boot_partition_durable(boot_deploy->android_boot->partition,
                                                                               raw_installed, clean_boot_image);
                raw.progress = Progress::applied;
                write_journal(root_, journal);
                unlink_file_or_link(host_path(root_, boot_deploy->android_boot->candidate_boot_image));
            }

            if (clean_boot_deploy) {
                const auto loader_index = manifest_index(boot_deploy->active_loader_entry);
                restore_index(loader_index, std::pair{std::span<const std::byte>(clean_loader), clean_loader_mode});
                const auto directory = host_path(root_, boot_deploy->candidate_directory);
                if (rmdir(directory.c_str()) != 0 && errno != ENOENT)
                    throw system_error("remove boot-deploy uninstall candidate directory", directory);
            }

            UninstallReport report{};
            for (std::size_t reverse = manifest_count; reverse > 0; --reverse) {
                const auto index = reverse - 1;
                restore_index(index);
                if (manifest->entries[index].original.kind == PreimageKind::absent)
                    ++report.removed;
                else
                    ++report.restored;
            }
            journal.phase = "cleanup";
            write_journal(root_, journal);
            uninstall_committed = true;
            cleanup_transaction_backups(root_, journal);
            cleanup_manifest_backups(root_, *manifest);
            unlink_file_or_link(host_path(root_, manifest_path));
            report.preserved_directories = remove_empty_directories(root_, manifest->created_dirs);
            static_cast<void>(remove_empty_directories(root_, journal.created_dirs));
            unlink_file_or_link(host_path(root_, journal_path));
            return report;
        } catch (...) {
            const auto failure = std::current_exception();
            if (!uninstall_committed) {
                try {
                    rollback(root_, journal, expected_owner_uid_);
                } catch (...) {
                }
            }
            std::rethrow_exception(failure);
        }
    }

    std::string render_status(const StatusReport &report) {
        std::string output =
            std::format("bootart install status\nroot: /\ninstalled: {}\n", report.installed ? "true" : "false");
        output += report.installed
                      ? std::format(
                            "provenance: installed-plan-version={} current-plan-version={} "
                            "installed-resource-set-version={} current-resource-set-version={} version-current=true\n"
                            "inventory: complete\n"
                            "image-verification: {}\n",
                            plan_version, plan_version, embedded::resource_set_version, embedded::resource_set_version,
                            report.image_verification.empty() ? "unresolved blocker=manifest-has-no-image-record"
                                                              : report.image_verification)
                      : "provenance: not-installed\ninventory: not-installed\nimage-verification: not-installed\n";
        output += std::format("recovery-required: {}\n", report.recovery_required ? "true" : "false");
        if (!report.transaction.empty())
            output += "transaction: " + report.transaction + "\n";
        for (const auto &file : report.files) {
            std::string_view state;
            switch (file.state) {
            case FileStatusState::exact:
                state = "exact";
                break;
            case FileStatusState::missing:
                state = "missing";
                break;
            case FileStatusState::mode_modified:
                state = "mode-modified";
                break;
            case FileStatusState::content_modified:
                state = "content-modified";
                break;
            case FileStatusState::type_modified:
                state = "type-modified";
                break;
            }
            output += std::format("file: {} expected-mode={:04o} expected-sha256={} state={} actual-sha256={}\n",
                                  file.path, file.expected_mode, file.expected_digest, state,
                                  file.actual_digest.empty() ? "-" : file.actual_digest);
        }
        return output;
    }

} // namespace bootart::install

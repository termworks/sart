#include "bootart/installer_backends.hpp"

#include "bootart/integration_patch.hpp"
#include "bootart/integration_resources.hpp"
#include "bootart/sha256.hpp"

#include <algorithm>
#include <charconv>
#include <limits>
#include <map>
#include <set>
#include <stdexcept>

namespace bootart::install {
    namespace {

        struct Member {
            std::string path;
            std::uint32_t mode;
            std::uint64_t uid;
            std::vector<std::byte> bytes;
        };

        std::uint64_t hex_field(std::span<const std::byte> input) {
            const std::string_view field(reinterpret_cast<const char *>(input.data()), input.size());
            std::uint64_t value{};
            const auto [end, error] = std::from_chars(field.data(), field.data() + field.size(), value, 16);
            if (error != std::errc{} || end != field.data() + field.size())
                throw std::runtime_error("invalid boot-deploy newc field");
            return value;
        }

        std::size_t align4(std::size_t value) {
            if (value > std::numeric_limits<std::size_t>::max() - 3)
                throw std::runtime_error("newc offset overflow");
            return (value + 3) & ~std::size_t(3);
        }

        std::optional<std::string> normalize(std::string_view name) {
            if (name == ".")
                return std::string(".");
            if (name.starts_with("./"))
                name.remove_prefix(2);
            if (name.empty() || name.size() > 4096 || name.front() == '/' || name.contains('\0'))
                return std::nullopt;
            std::string output;
            std::size_t offset = 0;
            while (offset < name.size()) {
                const auto end = name.find('/', offset);
                const auto part =
                    name.substr(offset, end == std::string_view::npos ? name.size() - offset : end - offset);
                if (part.empty() || part == "." || part == "..")
                    return std::nullopt;
                if (!output.empty())
                    output += '/';
                output.append(part);
                if (end == std::string_view::npos)
                    break;
                offset = end + 1;
            }
            return output;
        }

        std::pair<std::vector<Member>, std::uint64_t> parse_newc(std::span<const std::byte> candidate) {
            if (candidate.empty() || candidate.size() > max_candidate_bytes) {
                throw std::runtime_error("boot-deploy archive size is unsupported");
            }
            std::vector<Member> members;
            std::set<std::string> seen;
            std::uint64_t inspected = 0;
            bool trailer = false;
            std::size_t offset = 0;
            while (offset < candidate.size()) {
                if (std::ranges::all_of(candidate.subspan(offset), [](std::byte byte) { return byte == std::byte{}; }))
                    break;
                if (candidate.size() - offset < 110)
                    throw std::runtime_error("truncated boot-deploy newc header");
                const auto header = candidate.subspan(offset, 110);
                const std::string_view magic(reinterpret_cast<const char *>(header.data()), 6);
                if (magic != "070701" && magic != "070702")
                    throw std::runtime_error("boot-deploy archive is not newc");
                const auto mode = static_cast<std::uint32_t>(hex_field(header.subspan(14, 8)));
                const auto uid = hex_field(header.subspan(22, 8));
                const auto size = hex_field(header.subspan(54, 8));
                const auto name_size = hex_field(header.subspan(94, 8));
                if (name_size == 0 || name_size > 4096 || size > max_inspected_archive_bytes ||
                    name_size > candidate.size() - (offset + 110))
                    throw std::runtime_error("boot-deploy member exceeds a bound");
                const auto name_start = offset + 110;
                const auto name_bytes = candidate.subspan(name_start, static_cast<std::size_t>(name_size));
                if (name_bytes.back() != std::byte{} ||
                    std::ranges::find(name_bytes.first(name_bytes.size() - 1), std::byte{}) !=
                        name_bytes.first(name_bytes.size() - 1).end()) {
                    throw std::runtime_error("boot-deploy member name is not canonical");
                }
                const std::string_view name(reinterpret_cast<const char *>(name_bytes.data()), name_bytes.size() - 1);
                if (name == "TRAILER!!!") {
                    if (size != 0 || trailer)
                        throw std::runtime_error("malformed boot-deploy trailer");
                    trailer = true;
                    offset = align4(name_start + name_bytes.size());
                    continue;
                }
                if (trailer)
                    throw std::runtime_error("boot-deploy archive has members after trailer");
                const auto path = normalize(name);
                if (!path || !seen.insert(*path).second)
                    throw std::runtime_error("unsafe or duplicate boot-deploy path");
                if (members.size() >= max_archive_entries || size > max_inspected_archive_bytes - inspected) {
                    throw std::runtime_error("boot-deploy archive inspection bound exceeded");
                }
                inspected += size;
                const auto data_start = align4(name_start + name_bytes.size());
                if (data_start > candidate.size() || size > candidate.size() - data_start) {
                    throw std::runtime_error("truncated boot-deploy member data");
                }
                const auto data = candidate.subspan(data_start, static_cast<std::size_t>(size));
                members.push_back({*path, mode, uid, {data.begin(), data.end()}});
                offset = align4(data_start + data.size());
            }
            if (!trailer)
                throw std::runtime_error("boot-deploy archive has no trailer");
            return {std::move(members), inspected};
        }

        std::vector<std::byte> bytes(std::string_view value) {
            return {reinterpret_cast<const std::byte *>(value.data()),
                    reinterpret_cast<const std::byte *>(value.data() + value.size())};
        }

        std::string text(std::span<const std::byte> value) {
            return {reinterpret_cast<const char *>(value.data()), value.size()};
        }

    } // namespace

    ArchiveInspection inspect_mkinitfs_boot_deploy_archive(std::span<const std::byte> candidate,
                                                           std::span<const std::byte> expected_bootart) {
        const auto [members, inspected] = parse_newc(candidate);
        const std::map<std::string, std::pair<std::vector<std::byte>, std::uint16_t>> expected{
            {"usr/libexec/bootart/mkinitfs-boot-deploy-runtime",
             {bytes(integration::mkinitfs_boot_deploy::runtime_hook), 0755}},
            {"usr/libexec/bootart/mkinitfs-boot-deploy-fde",
             {bytes(integration::mkinitfs_boot_deploy::fde_wrapper), 0755}},
            {"usr/libexec/bootart/fde-unlock-stock",
             {bytes(integration::mkinitfs_boot_deploy::stock_fde_unlock), 0755}},
            {"usr/libexec/bootart/native-bin/unl0kr", {bytes(integration::mkinitfs_boot_deploy::native_unl0kr), 0755}},
            {"hooks-extra/50-bootart-start.sh", {bytes(integration::mkinitfs_boot_deploy::start_hook), 0755}},
            {"hooks-cleanup/90-bootart-handoff.sh", {bytes(integration::mkinitfs_boot_deploy::cleanup_hook), 0755}}};
        const std::set<std::string_view> expected_directories{"usr/libexec/bootart", "usr/libexec/bootart/native-bin"};
        std::set<std::string> found;
        std::optional<std::vector<std::byte>> bootart, init_functions;
        for (const auto &member : members) {
            const auto type = member.mode & 0170000;
            if (member.path.contains("bootart") && member.path != "usr/bin/bootart" &&
                !expected.contains(member.path) && !expected_directories.contains(member.path)) {
                throw std::runtime_error("foreign Bootart boot-deploy member");
            }
            if (member.path == ".") {
                if (type != 0040000 || member.uid != 0 || !member.bytes.empty() || (member.mode & 0022) != 0) {
                    throw std::runtime_error("unsafe boot-deploy archive root metadata");
                }
            } else if (expected_directories.contains(member.path)) {
                if (type != 0040000 || member.uid != 0 || !member.bytes.empty() || (member.mode & 0022) != 0) {
                    throw std::runtime_error("unsafe boot-deploy resource directory");
                }
            } else if (member.path == "usr/bin/bootart") {
                if (type != 0100000 || member.uid != 0 || (member.mode & 07777) != 0755) {
                    throw std::runtime_error("unsafe boot-deploy Bootart metadata");
                }
                bootart = member.bytes;
            } else if (member.path == "init_functions_2nd.sh") {
                if (type != 0100000 || member.uid != 0)
                    throw std::runtime_error("unsafe boot-deploy init functions");
                init_functions = member.bytes;
            } else if (const auto resource = expected.find(member.path); resource != expected.end()) {
                if (type != 0100000 || member.uid != 0 || (member.mode & 07777) != resource->second.second ||
                    member.bytes != resource->second.first) {
                    throw std::runtime_error("changed boot-deploy embedded resource");
                }
                found.insert(member.path);
            }
        }
        if (found.size() != expected.size() || !bootart ||
            *bootart != std::vector<std::byte>(expected_bootart.begin(), expected_bootart.end()) || !init_functions) {
            throw std::runtime_error("boot-deploy archive omits a required resource");
        }
        if (!integration::patch_boot_deploy_init_functions(text(*init_functions),
                                                           integration::reviewed_boot_deploy_initramfs_version)) {
            throw std::runtime_error("boot-deploy archive init functions differ from the contract");
        }
        return {sha256(expected_bootart), members.size(), inspected};
    }

    BootartFreeArchiveInspection
    inspect_bootart_free_mkinitfs_boot_deploy_archive(std::span<const std::byte> candidate) {
        const auto [members, inspected] = parse_newc(candidate);
        std::optional<std::vector<std::byte>> init_functions;
        bool fde = false, unl0kr = false, cryptsetup = false;
        for (const auto &member : members) {
            if (member.path.contains("bootart"))
                throw std::runtime_error("Bootart-free archive has Bootart residue");
            const auto type = member.mode & 0170000;
            const bool executable = type == 0100000 && member.uid == 0 && (member.mode & 0111) != 0;
            if (member.path == "init_functions_2nd.sh") {
                if (type != 0100000 || member.uid != 0 || (member.mode & 0022) != 0) {
                    throw std::runtime_error("unsafe Bootart-free init functions");
                }
                init_functions = member.bytes;
            } else if (member.path == "usr/bin/fde-unlock") {
                fde = executable;
            } else if (member.path == "usr/bin/unl0kr") {
                unl0kr = executable;
            } else if (member.path == "usr/bin/cryptsetup" || member.path == "usr/sbin/cryptsetup" ||
                       member.path == "sbin/cryptsetup") {
                cryptsetup = cryptsetup || executable;
            }
        }
        if (!init_functions ||
            !integration::patch_boot_deploy_init_functions(text(*init_functions),
                                                           integration::reviewed_boot_deploy_initramfs_version) ||
            !fde || !unl0kr || !cryptsetup) {
            throw std::runtime_error("Bootart-free archive lacks the stock unlock chain");
        }
        return {members.size(), inspected};
    }

} // namespace bootart::install

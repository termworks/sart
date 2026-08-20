#include "bootart/integration_patch.hpp"

#include "bootart/integration_resources.hpp"

namespace bootart::integration {
    namespace {

        constexpr std::string_view mkinitfs_version_record = "VERSION=3.14.0-r0\n";
        constexpr std::string_view mkinitfs_early_anchor =
            "# check if root=... was set\nif [ -n \"$KOPT_root\" ]; then\n";
        constexpr std::string_view mkinitfs_handoff_anchor = "\t\t\t$MOCK mount -o move \"$DIR\" \"$sysroot/$DIR\"\n"
                                                             "\t\tfi\n"
                                                             "\tdone\n"
                                                             "\t$MOCK sync\n";

        constexpr std::string_view stock_unlock_function = R"BOOTART(unlock_root_partition() {
	command -v cryptsetup >/dev/null || return
	if cryptsetup isLuks "$PMOS_ROOT"; then
		splash_hide
		tried=0
		until cryptsetup status root | grep -qwi active; do
			fde-unlock "$PMOS_ROOT" "$tried"
			tried=$((tried + 1))
		done
		PMOS_ROOT=/dev/mapper/root
		splash_set_message "Loading"
	fi
}
)BOOTART";

        std::size_t count(std::string_view input, std::string_view needle) {
            std::size_t result = 0;
            for (std::size_t at = 0; (at = input.find(needle, at)) != std::string_view::npos; at += needle.size()) {
                ++result;
            }
            return result;
        }

        std::string replace_once(std::string_view input, std::string_view needle, std::string_view replacement) {
            std::string result(input);
            const auto at = result.find(needle);
            if (at != std::string::npos) {
                result.replace(at, needle.size(), replacement);
            }
            return result;
        }

        std::expected<std::string, PatchError> patch_clean_mkinitfs(std::string_view input) {
            if (count(input, mkinitfs_version_record) != 1) {
                return std::unexpected(PatchError::unsupported_version);
            }
            if (count(input, mkinitfs_early_anchor) != 1) {
                return std::unexpected(PatchError::ambiguous_early_insertion_point);
            }
            if (count(input, mkinitfs_handoff_anchor) != 1) {
                return std::unexpected(PatchError::ambiguous_handoff_insertion_point);
            }
            std::string early_replacement(mkinitfs_early_anchor);
            early_replacement += '\n';
            early_replacement += mkinitfs::early_call_snippet;
            auto result = replace_once(input, mkinitfs_early_anchor, early_replacement);
            std::string handoff_replacement(mkinitfs_handoff_anchor.substr(
                0, mkinitfs_handoff_anchor.size() - std::string_view("\t$MOCK sync\n").size()));
            handoff_replacement += mkinitfs::handoff_call_snippet;
            handoff_replacement += "\t$MOCK sync\n";
            return replace_once(result, mkinitfs_handoff_anchor, handoff_replacement);
        }

        std::string patched_unlock_function() {
            return replace_once(stock_unlock_function, "\t\t\tfde-unlock \"$PMOS_ROOT\" \"$tried\"\n",
                                mkinitfs_boot_deploy::fde_call_snippet);
        }

    } // namespace

    std::expected<std::string, PatchError> patch_mkinitfs_init(std::string_view input) {
        const auto early_count = count(input, mkinitfs::early_call_snippet);
        const auto handoff_count = count(input, mkinitfs::handoff_call_snippet);
        if (early_count == 0 && handoff_count == 0 && !input.contains("# bootart:begin mkinitfs-") &&
            !input.contains("# bootart:end mkinitfs-")) {
            return patch_clean_mkinitfs(input);
        }
        if (early_count != 1 || handoff_count != 1) {
            return std::unexpected(PatchError::partial_managed_state);
        }
        std::string early_insertion("\n");
        early_insertion += mkinitfs::early_call_snippet;
        auto clean = replace_once(input, early_insertion, "");
        clean = replace_once(clean, mkinitfs::handoff_call_snippet, "");
        auto expected = patch_clean_mkinitfs(clean);
        if (!expected) {
            return std::unexpected(expected.error());
        }
        if (*expected != input) {
            return std::unexpected(PatchError::managed_content_mismatch);
        }
        return std::string(input);
    }

    std::expected<std::string, PatchError> patch_boot_deploy_init_functions(std::string_view input,
                                                                            std::string_view version) {
        if (version != reviewed_boot_deploy_initramfs_version) {
            return std::unexpected(PatchError::unsupported_version);
        }
        const auto patched = patched_unlock_function();
        constexpr std::string_view begin = "# bootart:begin mkinitfs-boot-deploy-fde-v1";
        constexpr std::string_view end = "# bootart:end mkinitfs-boot-deploy-fde-v1";
        const auto stock_count = count(input, stock_unlock_function);
        const auto patched_count = count(input, patched);
        const auto begin_count = count(input, begin);
        const auto end_count = count(input, end);
        if (stock_count == 1 && patched_count == 0 && begin_count == 0 && end_count == 0) {
            return replace_once(input, stock_unlock_function, patched);
        }
        if (stock_count == 0 && patched_count == 1 && begin_count == 1 && end_count == 1) {
            auto clean = replace_once(input, patched, stock_unlock_function);
            auto expected = replace_once(clean, stock_unlock_function, patched);
            if (expected == input) {
                return std::string(input);
            }
            return std::unexpected(PatchError::managed_content_mismatch);
        }
        if (begin_count == 0 && end_count == 0) {
            return std::unexpected(PatchError::ambiguous_unlock_function);
        }
        return std::unexpected(PatchError::partial_managed_state);
    }

} // namespace bootart::integration

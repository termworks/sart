#pragma once

#include <string_view>

namespace bootart::integration::systemd {
    extern const std::string_view console_agent_drop_in;
    extern const std::string_view start_unit;
    extern const std::string_view show_unit;
    extern const std::string_view switch_root_unit;
    extern const std::string_view quit_unit;
    extern const std::string_view quit_wait_unit;
} // namespace bootart::integration::systemd

namespace bootart::integration::openrc {
    extern const std::string_view supervisor_script;
    extern const std::string_view quit_script;
} // namespace bootart::integration::openrc

namespace bootart::integration::dracut {
    extern const std::string_view systemd_config;
    extern const std::string_view systemd_module_setup;
    extern const std::string_view classic_module_setup;
    extern const std::string_view classic_start_hook;
    extern const std::string_view classic_askpass_patch_hook;
    extern const std::string_view classic_askpass_override;
    extern const std::string_view classic_pre_pivot_hook;
} // namespace bootart::integration::dracut

namespace bootart::integration::initramfs_tools {
    extern const std::string_view build_hook;
    extern const std::string_view askpass_wrapper;
    extern const std::string_view early_hook;
    extern const std::string_view bottom_hook;
} // namespace bootart::integration::initramfs_tools

namespace bootart::integration::mkinitcpio {
    extern const std::string_view install_hook;
    extern const std::string_view runtime_hook;
    extern const std::string_view plymouth_bridge;
} // namespace bootart::integration::mkinitcpio

namespace bootart::integration::mkinitfs {
    extern const std::string_view feature_files;
    extern const std::string_view runtime_hook;
    extern const std::string_view findfs_wrapper;
    extern const std::string_view early_call_snippet;
    extern const std::string_view handoff_call_snippet;
} // namespace bootart::integration::mkinitfs

namespace bootart::integration::mkinitfs_boot_deploy {
    extern const std::string_view files_extra;
    extern const std::string_view kernel_cmdline_override;
    extern const std::string_view apk_commit_hook;
    extern const std::string_view runtime_hook;
    extern const std::string_view fde_wrapper;
    extern const std::string_view stock_fde_unlock;
    extern const std::string_view native_unl0kr;
    extern const std::string_view start_hook;
    extern const std::string_view cleanup_hook;
    extern const std::string_view fde_call_snippet;
} // namespace bootart::integration::mkinitfs_boot_deploy

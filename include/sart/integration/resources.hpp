#pragma once

#include <string_view>

namespace sart::integration::systemd {
    extern const std::string_view console_agent_drop_in;
    extern const std::string_view start_unit;
    extern const std::string_view show_unit;
    extern const std::string_view switch_root_unit;
    extern const std::string_view quit_unit;
    extern const std::string_view quit_wait_unit;
} // namespace sart::integration::systemd

namespace sart::integration::openrc {
    extern const std::string_view supervisor_script;
    extern const std::string_view quit_script;
} // namespace sart::integration::openrc

namespace sart::integration::dracut {
    extern const std::string_view systemd_config;
    extern const std::string_view systemd_module_setup;
    extern const std::string_view classic_module_setup;
    extern const std::string_view classic_start_hook;
    extern const std::string_view classic_askpass_patch_hook;
    extern const std::string_view classic_askpass_override;
    extern const std::string_view classic_pre_pivot_hook;
} // namespace sart::integration::dracut

namespace sart::integration::initramfs_tools {
    extern const std::string_view build_hook;
    extern const std::string_view askpass_wrapper;
    extern const std::string_view early_hook;
    extern const std::string_view bottom_hook;
} // namespace sart::integration::initramfs_tools

namespace sart::integration::mkinitcpio {
    extern const std::string_view install_hook;
    extern const std::string_view runtime_hook;
    extern const std::string_view plymouth_bridge;
} // namespace sart::integration::mkinitcpio

namespace sart::integration::mkinitfs {
    extern const std::string_view feature_files;
    extern const std::string_view runtime_hook;
    extern const std::string_view findfs_wrapper;
    extern const std::string_view early_call_snippet;
    extern const std::string_view handoff_call_snippet;
} // namespace sart::integration::mkinitfs

namespace sart::integration::mkinitfs_boot_deploy {
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
} // namespace sart::integration::mkinitfs_boot_deploy

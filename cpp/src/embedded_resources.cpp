#include "bootart/embedded.hpp"

#include "bootart/integration_resources.hpp"

#include <stdexcept>

namespace bootart::embedded {
    namespace {

        TemplateMaterialization file(std::string_view path, std::uint16_t mode) {
            return {MaterializationKind::file, path, mode, {}, {}};
        }

        TemplateMaterialization service(std::string_view path, std::string_view runlevel) {
            return {MaterializationKind::openrc_service, path, 0755, runlevel, {}};
        }

        TemplateMaterialization snippet(std::string_view target, std::string_view insertion_point) {
            return {MaterializationKind::managed_snippet, target, 0, {}, insertion_point};
        }

    } // namespace

    std::string_view template_name(TemplateId id) {
        switch (id) {
        case TemplateId::systemd_start_unit:
            return "systemd.start-unit";
        case TemplateId::systemd_show_unit:
            return "systemd.show-unit";
        case TemplateId::systemd_switch_root_unit:
            return "systemd.switch-root-unit";
        case TemplateId::systemd_quit_unit:
            return "systemd.quit-unit";
        case TemplateId::systemd_quit_wait_unit:
            return "systemd.quit-wait-unit";
        case TemplateId::systemd_console_agent_drop_in:
            return "systemd.console-agent-drop-in";
        case TemplateId::dracut_systemd_config:
            return "dracut.systemd-config";
        case TemplateId::dracut_systemd_module_setup:
            return "dracut.systemd-module-setup";
        case TemplateId::dracut_classic_module_setup:
            return "dracut.classic-module-setup";
        case TemplateId::dracut_classic_start_hook:
            return "dracut.classic-start-hook";
        case TemplateId::dracut_classic_askpass_patch_hook:
            return "dracut.classic-askpass-patch-hook";
        case TemplateId::dracut_classic_askpass_override:
            return "dracut.classic-askpass-override";
        case TemplateId::dracut_classic_pre_pivot_hook:
            return "dracut.classic-pre-pivot-hook";
        case TemplateId::mkinitcpio_install_hook:
            return "mkinitcpio.install-hook";
        case TemplateId::mkinitcpio_runtime_hook:
            return "mkinitcpio.runtime-hook";
        case TemplateId::mkinitcpio_plymouth_bridge:
            return "mkinitcpio.plymouth-bridge";
        case TemplateId::initramfs_tools_build_hook:
            return "initramfs-tools.build-hook";
        case TemplateId::initramfs_tools_askpass_wrapper:
            return "initramfs-tools.askpass-wrapper";
        case TemplateId::initramfs_tools_early_hook:
            return "initramfs-tools.early-hook";
        case TemplateId::initramfs_tools_bottom_hook:
            return "initramfs-tools.bottom-hook";
        case TemplateId::mkinitfs_feature_files:
            return "mkinitfs.feature-files";
        case TemplateId::mkinitfs_runtime_hook:
            return "mkinitfs.runtime-hook";
        case TemplateId::mkinitfs_findfs_wrapper:
            return "mkinitfs.findfs-wrapper";
        case TemplateId::mkinitfs_early_call_snippet:
            return "mkinitfs.early-call-snippet";
        case TemplateId::mkinitfs_handoff_call_snippet:
            return "mkinitfs.handoff-call-snippet";
        case TemplateId::mkinitfs_boot_deploy_files:
            return "mkinitfs-boot-deploy.files-extra";
        case TemplateId::mkinitfs_boot_deploy_kernel_cmdline:
            return "mkinitfs-boot-deploy.kernel-cmdline-override";
        case TemplateId::mkinitfs_boot_deploy_apk_commit_hook:
            return "mkinitfs-boot-deploy.apk-commit-hook";
        case TemplateId::mkinitfs_boot_deploy_runtime:
            return "mkinitfs-boot-deploy.runtime-hook";
        case TemplateId::mkinitfs_boot_deploy_fde_wrapper:
            return "mkinitfs-boot-deploy.fde-wrapper";
        case TemplateId::mkinitfs_boot_deploy_stock_fde:
            return "mkinitfs-boot-deploy.stock-fde-unlock";
        case TemplateId::mkinitfs_boot_deploy_native_unl0kr:
            return "mkinitfs-boot-deploy.native-unl0kr";
        case TemplateId::mkinitfs_boot_deploy_start_hook:
            return "mkinitfs-boot-deploy.start-hook";
        case TemplateId::mkinitfs_boot_deploy_cleanup_hook:
            return "mkinitfs-boot-deploy.cleanup-hook";
        case TemplateId::mkinitfs_boot_deploy_fde_call_snippet:
            return "mkinitfs-boot-deploy.fde-call-snippet";
        case TemplateId::openrc_supervisor_script:
            return "openrc.supervisor-script";
        case TemplateId::openrc_quit_script:
            return "openrc.quit-script";
        }
        throw std::invalid_argument("unknown embedded template");
    }

    TemplateResource template_resource(TemplateId id) {
        using namespace integration;
        TemplateMaterialization target{};
        std::string_view contents;
        switch (id) {
        case TemplateId::systemd_start_unit:
            target = file("/usr/lib/systemd/system/bootart-start.service", 0644);
            contents = systemd::start_unit;
            break;
        case TemplateId::systemd_show_unit:
            target = file("/usr/lib/systemd/system/bootart-show.service", 0644);
            contents = systemd::show_unit;
            break;
        case TemplateId::systemd_switch_root_unit:
            target = file("/usr/lib/systemd/system/bootart-switch-root.service", 0644);
            contents = systemd::switch_root_unit;
            break;
        case TemplateId::systemd_quit_unit:
            target = file("/usr/lib/systemd/system/bootart-quit.service", 0644);
            contents = systemd::quit_unit;
            break;
        case TemplateId::systemd_quit_wait_unit:
            target = file("/usr/lib/systemd/system/bootart-quit-wait.service", 0644);
            contents = systemd::quit_wait_unit;
            break;
        case TemplateId::systemd_console_agent_drop_in:
            target = file("/usr/lib/systemd/system/systemd-ask-password-console.service.d/50-bootart.conf", 0644);
            contents = systemd::console_agent_drop_in;
            break;
        case TemplateId::dracut_systemd_config:
            target = file("/etc/dracut.conf.d/60-bootart-systemd.conf", 0644);
            contents = dracut::systemd_config;
            break;
        case TemplateId::dracut_systemd_module_setup:
            target = file("/usr/lib/dracut/modules.d/60bootart-systemd/module-setup.sh", 0755);
            contents = dracut::systemd_module_setup;
            break;
        case TemplateId::dracut_classic_module_setup:
            target = file("/usr/lib/dracut/modules.d/60bootart-classic/module-setup.sh", 0755);
            contents = dracut::classic_module_setup;
            break;
        case TemplateId::dracut_classic_start_hook:
            target = file("/usr/lib/dracut/modules.d/60bootart-classic/bootart-start.sh", 0755);
            contents = dracut::classic_start_hook;
            break;
        case TemplateId::dracut_classic_askpass_patch_hook:
            target = file("/usr/lib/dracut/modules.d/60bootart-classic/bootart-askpass-patch.sh", 0755);
            contents = dracut::classic_askpass_patch_hook;
            break;
        case TemplateId::dracut_classic_askpass_override:
            target = file("/usr/lib/dracut/modules.d/60bootart-classic/bootart-askpass-lib.sh", 0644);
            contents = dracut::classic_askpass_override;
            break;
        case TemplateId::dracut_classic_pre_pivot_hook:
            target = file("/usr/lib/dracut/modules.d/60bootart-classic/bootart-pre-pivot.sh", 0755);
            contents = dracut::classic_pre_pivot_hook;
            break;
        case TemplateId::mkinitcpio_install_hook:
            target = file("/usr/lib/initcpio/install/bootart", 0755);
            contents = mkinitcpio::install_hook;
            break;
        case TemplateId::mkinitcpio_runtime_hook:
            target = file("/usr/lib/initcpio/hooks/bootart", 0755);
            contents = mkinitcpio::runtime_hook;
            break;
        case TemplateId::mkinitcpio_plymouth_bridge:
            target = file("/usr/lib/bootart/mkinitcpio-plymouth", 0755);
            contents = mkinitcpio::plymouth_bridge;
            break;
        case TemplateId::initramfs_tools_build_hook:
            target = file("/usr/share/initramfs-tools/hooks/bootart", 0755);
            contents = initramfs_tools::build_hook;
            break;
        case TemplateId::initramfs_tools_askpass_wrapper:
            target = file("/usr/lib/bootart/initramfs-tools-askpass", 0755);
            contents = initramfs_tools::askpass_wrapper;
            break;
        case TemplateId::initramfs_tools_early_hook:
            target = file("/usr/share/initramfs-tools/scripts/init-top/bootart", 0755);
            contents = initramfs_tools::early_hook;
            break;
        case TemplateId::initramfs_tools_bottom_hook:
            target = file("/usr/share/initramfs-tools/scripts/init-bottom/bootart", 0755);
            contents = initramfs_tools::bottom_hook;
            break;
        case TemplateId::mkinitfs_feature_files:
            target = file("/etc/mkinitfs/features.d/bootart.files", 0644);
            contents = mkinitfs::feature_files;
            break;
        case TemplateId::mkinitfs_runtime_hook:
            target = file("/usr/libexec/bootart/mkinitfs-runtime", 0755);
            contents = mkinitfs::runtime_hook;
            break;
        case TemplateId::mkinitfs_findfs_wrapper:
            target = file("/usr/libexec/bootart/mkinitfs-findfs", 0755);
            contents = mkinitfs::findfs_wrapper;
            break;
        case TemplateId::mkinitfs_early_call_snippet:
            target = snippet("/usr/share/mkinitfs/initramfs-init", "post-boot-drivers-before-root-discovery");
            contents = mkinitfs::early_call_snippet;
            break;
        case TemplateId::mkinitfs_handoff_call_snippet:
            target = snippet("/usr/share/mkinitfs/initramfs-init", "post-initramfs-mount-move-before-switch-root");
            contents = mkinitfs::handoff_call_snippet;
            break;
        case TemplateId::mkinitfs_boot_deploy_files:
            target = file("/etc/mkinitfs/files-extra/bootart", 0644);
            contents = mkinitfs_boot_deploy::files_extra;
            break;
        case TemplateId::mkinitfs_boot_deploy_kernel_cmdline:
            target = file("/etc/kernel-cmdline.d/90-bootart.conf", 0644);
            contents = mkinitfs_boot_deploy::kernel_cmdline_override;
            break;
        case TemplateId::mkinitfs_boot_deploy_apk_commit_hook:
            target = file("/etc/apk/commit_hooks.d/95-bootart-raw-boot", 0755);
            contents = mkinitfs_boot_deploy::apk_commit_hook;
            break;
        case TemplateId::mkinitfs_boot_deploy_runtime:
            target = file("/usr/libexec/bootart/mkinitfs-boot-deploy-runtime", 0755);
            contents = mkinitfs_boot_deploy::runtime_hook;
            break;
        case TemplateId::mkinitfs_boot_deploy_fde_wrapper:
            target = file("/usr/libexec/bootart/mkinitfs-boot-deploy-fde", 0755);
            contents = mkinitfs_boot_deploy::fde_wrapper;
            break;
        case TemplateId::mkinitfs_boot_deploy_stock_fde:
            target = file("/usr/libexec/bootart/fde-unlock-stock", 0755);
            contents = mkinitfs_boot_deploy::stock_fde_unlock;
            break;
        case TemplateId::mkinitfs_boot_deploy_native_unl0kr:
            target = file("/usr/libexec/bootart/native-bin/unl0kr", 0755);
            contents = mkinitfs_boot_deploy::native_unl0kr;
            break;
        case TemplateId::mkinitfs_boot_deploy_start_hook:
            target = file("/etc/mkinitfs/hooks-extra/50-bootart-start.sh", 0755);
            contents = mkinitfs_boot_deploy::start_hook;
            break;
        case TemplateId::mkinitfs_boot_deploy_cleanup_hook:
            target = file("/etc/mkinitfs/hooks-cleanup/90-bootart-handoff.sh", 0755);
            contents = mkinitfs_boot_deploy::cleanup_hook;
            break;
        case TemplateId::mkinitfs_boot_deploy_fde_call_snippet:
            target =
                snippet("/usr/share/initramfs/init_functions_2nd.sh", "reviewed-unlock-root-password-producer-call");
            contents = mkinitfs_boot_deploy::fde_call_snippet;
            break;
        case TemplateId::openrc_supervisor_script:
            target = service("/etc/init.d/bootart", "boot");
            contents = openrc::supervisor_script;
            break;
        case TemplateId::openrc_quit_script:
            target = service("/etc/init.d/bootart-quit", "default");
            contents = openrc::quit_script;
            break;
        }
        return {id, target, contents, true};
    }

} // namespace bootart::embedded

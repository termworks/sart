#include "sart/install/adapter.hpp"

#include <stdexcept>

namespace sart::install {
    namespace {

        using embedded::TemplateId;

        constexpr std::array dracut_systemd_resources{
            TemplateId::systemd_start_unit,       TemplateId::systemd_show_unit,
            TemplateId::systemd_switch_root_unit, TemplateId::systemd_console_agent_drop_in,
            TemplateId::dracut_systemd_config,    TemplateId::dracut_systemd_module_setup,
        };
        constexpr std::array systemd_resources{
            TemplateId::systemd_quit_unit,
            TemplateId::systemd_quit_wait_unit,
        };
        constexpr std::array dracut_classic_resources{
            TemplateId::dracut_classic_module_setup,       TemplateId::dracut_classic_start_hook,
            TemplateId::dracut_classic_askpass_patch_hook, TemplateId::dracut_classic_askpass_override,
            TemplateId::dracut_classic_pre_pivot_hook,
        };
        constexpr std::array initramfs_tools_resources{
            TemplateId::initramfs_tools_build_hook,
            TemplateId::initramfs_tools_askpass_wrapper,
            TemplateId::initramfs_tools_early_hook,
            TemplateId::initramfs_tools_bottom_hook,
        };
        constexpr std::array mkinitcpio_resources{
            TemplateId::mkinitcpio_install_hook,
            TemplateId::mkinitcpio_runtime_hook,
            TemplateId::mkinitcpio_plymouth_bridge,
        };
        constexpr std::array mkinitfs_resources{
            TemplateId::mkinitfs_feature_files,        TemplateId::mkinitfs_runtime_hook,
            TemplateId::mkinitfs_findfs_wrapper,       TemplateId::mkinitfs_early_call_snippet,
            TemplateId::mkinitfs_handoff_call_snippet,
        };
        constexpr std::array boot_deploy_resources{
            TemplateId::mkinitfs_boot_deploy_files,           TemplateId::mkinitfs_boot_deploy_kernel_cmdline,
            TemplateId::mkinitfs_boot_deploy_apk_commit_hook, TemplateId::mkinitfs_boot_deploy_runtime,
            TemplateId::mkinitfs_boot_deploy_fde_wrapper,     TemplateId::mkinitfs_boot_deploy_stock_fde,
            TemplateId::mkinitfs_boot_deploy_native_unl0kr,   TemplateId::mkinitfs_boot_deploy_start_hook,
            TemplateId::mkinitfs_boot_deploy_cleanup_hook,    TemplateId::mkinitfs_boot_deploy_fde_call_snippet,
        };
        constexpr std::array openrc_resources{
            TemplateId::openrc_supervisor_script,
            TemplateId::openrc_quit_script,
        };

        const std::array adapters{
            AdapterMetadata{AdapterId::dracut_systemd, "dracut-systemd-initramfs", AdapterKind::initramfs_runtime,
                            PasswordBrokerStatus::integrated_unproven, dracut_systemd_resources,
                            "requires the exact proven dracut-systemd and systemd capability contract"},
            AdapterMetadata{AdapterId::systemd_real_root, "systemd-real-root", AdapterKind::real_root_supervisor,
                            PasswordBrokerStatus::not_applicable, systemd_resources,
                            "requires an exact proven initramfs and systemd pair"},
            AdapterMetadata{AdapterId::dracut_classic, "dracut-classic-initramfs", AdapterKind::initramfs_runtime,
                            PasswordBrokerStatus::integrated_unproven, dracut_classic_resources,
                            "classic dracut and OpenRC remain VM-unproven"},
            AdapterMetadata{AdapterId::initramfs_tools_busybox, "initramfs-tools-busybox",
                            AdapterKind::initramfs_runtime, PasswordBrokerStatus::integrated_unproven,
                            initramfs_tools_resources, "requires the exact proven initramfs-tools contract"},
            AdapterMetadata{AdapterId::mkinitcpio_busybox, "mkinitcpio-busybox", AdapterKind::initramfs_runtime,
                            PasswordBrokerStatus::integrated_unproven, mkinitcpio_resources,
                            "requires the exact proven mkinitcpio contract"},
            AdapterMetadata{AdapterId::mkinitfs_busybox, "mkinitfs-busybox", AdapterKind::initramfs_runtime,
                            PasswordBrokerStatus::integrated_unproven, mkinitfs_resources,
                            "requires the reviewed mkinitfs source contract"},
            AdapterMetadata{AdapterId::mkinitfs_boot_deploy, "mkinitfs-boot-deploy-initramfs",
                            AdapterKind::initramfs_runtime, PasswordBrokerStatus::integrated_unproven,
                            boot_deploy_resources, "requires the exact mkinitfs and boot-deploy contract"},
            AdapterMetadata{AdapterId::openrc_real_root, "openrc-real-root", AdapterKind::real_root_supervisor,
                            PasswordBrokerStatus::not_applicable, openrc_resources,
                            "requires an exact proven initramfs and OpenRC pair"},
        };

        constexpr std::array dracut_systemd_gates{
            std::string_view("make vm-test-lifecycle-dracut-systemd"),
            std::string_view("make vm-test-install-dracut-systemd"),
            std::string_view("make vm-test-password-dracut-systemd"),
            std::string_view("make vm-test-recovery-dracut-systemd"),
            std::string_view("make vm-test-uninstall-dracut-systemd"),
            std::string_view("make vm-test-kernel-update-dracut-systemd"),
        };
        constexpr std::array initramfs_tools_gates{
            std::string_view("make vm-test-lifecycle-initramfs-tools"),
            std::string_view("make vm-test-install-initramfs-tools"),
            std::string_view("make vm-test-password-initramfs-tools"),
            std::string_view("make vm-test-recovery-initramfs-tools"),
            std::string_view("make vm-test-uninstall-initramfs-tools"),
            std::string_view("make vm-test-kernel-update-initramfs-tools"),
        };
        constexpr std::array mkinitcpio_gates{
            std::string_view("make vm-test-lifecycle-mkinitcpio"),
            std::string_view("make vm-test-install-mkinitcpio"),
            std::string_view("make vm-test-password-mkinitcpio"),
            std::string_view("make vm-test-recovery-mkinitcpio"),
            std::string_view("make vm-test-uninstall-mkinitcpio"),
            std::string_view("make vm-test-kernel-update-mkinitcpio"),
        };
        constexpr std::array dracut_classic_gates{
            std::string_view("make vm-test-lifecycle-dracut-classic"),
            std::string_view("make vm-test-install-dracut-classic"),
            std::string_view("make vm-test-password-dracut-classic"),
            std::string_view("make vm-test-recovery-dracut-classic"),
            std::string_view("make vm-test-uninstall-dracut-classic"),
            std::string_view("make vm-test-kernel-update-dracut-classic"),
        };
        constexpr std::array mkinitfs_gates{
            std::string_view("make vm-test-lifecycle-mkinitfs-openrc"),
            std::string_view("make vm-test-install-mkinitfs-openrc"),
            std::string_view("make vm-test-password-mkinitfs-openrc"),
            std::string_view("make vm-test-recovery-mkinitfs-openrc"),
            std::string_view("make vm-test-uninstall-mkinitfs-openrc"),
            std::string_view("make vm-test-kernel-update-mkinitfs-openrc"),
        };
        constexpr std::array boot_deploy_openrc_gates{
            std::string_view("make vm-test-lifecycle-mkinitfs-boot-deploy-openrc"),
            std::string_view("make vm-test-install-mkinitfs-boot-deploy-openrc"),
            std::string_view("make vm-test-password-mkinitfs-boot-deploy-openrc"),
            std::string_view("make vm-test-recovery-mkinitfs-boot-deploy-openrc"),
            std::string_view("make vm-test-uninstall-mkinitfs-boot-deploy-openrc"),
            std::string_view("make vm-test-kernel-update-mkinitfs-boot-deploy-openrc"),
        };
        constexpr std::array boot_deploy_systemd_gates{
            std::string_view("make vm-test-lifecycle-mkinitfs-boot-deploy-systemd"),
            std::string_view("make vm-test-install-mkinitfs-boot-deploy-systemd"),
            std::string_view("make vm-test-password-mkinitfs-boot-deploy-systemd"),
            std::string_view("make vm-test-recovery-mkinitfs-boot-deploy-systemd"),
            std::string_view("make vm-test-uninstall-mkinitfs-boot-deploy-systemd"),
            std::string_view("make vm-test-kernel-update-mkinitfs-boot-deploy-systemd"),
        };

        const std::array pairs{
            AdapterPairMetadata{"dracut-systemd", AdapterId::dracut_systemd, AdapterId::systemd_real_root,
                                SupportStatus::proven_supported, dracut_systemd_gates,
                                "requires exact live dracut-systemd and systemd capabilities"},
            AdapterPairMetadata{"initramfs-tools", AdapterId::initramfs_tools_busybox, AdapterId::systemd_real_root,
                                SupportStatus::proven_supported, initramfs_tools_gates,
                                "requires exact live initramfs-tools and systemd capabilities"},
            AdapterPairMetadata{"mkinitcpio", AdapterId::mkinitcpio_busybox, AdapterId::systemd_real_root,
                                SupportStatus::proven_supported, mkinitcpio_gates,
                                "requires exact live mkinitcpio and systemd capabilities"},
            AdapterPairMetadata{"dracut-classic", AdapterId::dracut_classic, AdapterId::openrc_real_root,
                                SupportStatus::experimental_unproven, dracut_classic_gates,
                                "classic dracut and OpenRC are not VM-proven"},
            AdapterPairMetadata{"mkinitfs-openrc", AdapterId::mkinitfs_busybox, AdapterId::openrc_real_root,
                                SupportStatus::proven_supported, mkinitfs_gates,
                                "requires exact live mkinitfs, OpenRC, and extlinux capabilities"},
            AdapterPairMetadata{"mkinitfs-boot-deploy-openrc", AdapterId::mkinitfs_boot_deploy,
                                AdapterId::openrc_real_root, SupportStatus::proven_supported, boot_deploy_openrc_gates,
                                "requires exact live mkinitfs, boot-deploy, OpenRC, and BLS capabilities"},
            AdapterPairMetadata{"mkinitfs-boot-deploy-systemd", AdapterId::mkinitfs_boot_deploy,
                                AdapterId::systemd_real_root, SupportStatus::proven_supported,
                                boot_deploy_systemd_gates,
                                "requires exact live mkinitfs, boot-deploy, systemd, and BLS capabilities"},
        };

    } // namespace

    const AdapterMetadata &adapter_metadata(AdapterId id) {
        for (const auto &adapter : adapters) {
            if (adapter.id == id) {
                return adapter;
            }
        }
        throw std::invalid_argument("unknown adapter");
    }

    std::span<const AdapterPairMetadata> adapter_pairs() { return pairs; }

    const AdapterPairMetadata *adapter_pair(AdapterId initramfs, AdapterId real_root) {
        for (const auto &pair : pairs) {
            if (pair.initramfs == initramfs && pair.real_root == real_root) {
                return &pair;
            }
        }
        return nullptr;
    }

    std::string_view adapter_name(AdapterId id) { return adapter_metadata(id).name; }

} // namespace sart::install

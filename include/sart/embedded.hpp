#pragma once

#include <array>
#include <cstdint>
#include <string_view>

namespace sart::embedded {

    inline constexpr std::string_view default_art = R"SART(              ▄▄▄▄▄▄▄▄              
         ▄▄██████████████▄▄         
      ▄██████████████████████▄      
    ▄██████████████████████████▄    
  ▄█▀▄████████████████████████▄▀█▄  
 ▄█  ██████████████████████████  █▄ 
▄█▀ ▄██████████████████████████▄ ▀█▄
█▀  ████████████████████████████  ▀█
  ▄██████████████████████████████▄  
████████████████████████████████████
████████████████████████████████████
▀██▀  ▀▀████████████████████▀▀  ▀██▀
 ██       ▀██▀████████▀██▀       ██ 
  ██        ▀█ ██████ █▀        ██  
   ██▄        █ ████ █        ▄██   
    ███▄▄▄▄    █ ██ █    ▄▄▄▄███    
     ▀▀▀▀▀████▄██████▄████▀▀▀▀▀     
        █▄ █████▄██▄█████ ▄█        
        ██▄ ████████████ ▄██        
         ▀█████▀▄▄▄▄▀█████▀         
           ▀▀██████████▀▀           
              ▀██████▀              
)SART";

    inline constexpr std::string_view small_art = R"SART( ___  ____ ___  ____ ___ 
|__] |  |  |  |__|  |  
|__] |__|  |  |  |  |  
)SART";

    inline constexpr std::uint16_t resource_set_version = 13;

    inline constexpr std::string_view default_config = R"SART(schema=sart.config
version=1
runtime_dir=/run/sart
mode=boot
password_broker=none
vt=open-query
frames_per_second=30
animation_cycle_ms=2500
seed=42
no_color=false
control_protocol=1
)SART";

    enum class TemplateId : std::uint8_t {
        systemd_start_unit,
        systemd_show_unit,
        systemd_switch_root_unit,
        systemd_quit_unit,
        systemd_quit_wait_unit,
        systemd_console_agent_drop_in,
        dracut_systemd_config,
        dracut_systemd_module_setup,
        dracut_classic_module_setup,
        dracut_classic_start_hook,
        dracut_classic_askpass_patch_hook,
        dracut_classic_askpass_override,
        dracut_classic_pre_pivot_hook,
        mkinitcpio_install_hook,
        mkinitcpio_runtime_hook,
        mkinitcpio_plymouth_bridge,
        initramfs_tools_build_hook,
        initramfs_tools_askpass_wrapper,
        initramfs_tools_early_hook,
        initramfs_tools_bottom_hook,
        mkinitfs_feature_files,
        mkinitfs_runtime_hook,
        mkinitfs_findfs_wrapper,
        mkinitfs_early_call_snippet,
        mkinitfs_handoff_call_snippet,
        mkinitfs_boot_deploy_files,
        mkinitfs_boot_deploy_kernel_cmdline,
        mkinitfs_boot_deploy_apk_commit_hook,
        mkinitfs_boot_deploy_runtime,
        mkinitfs_boot_deploy_fde_wrapper,
        mkinitfs_boot_deploy_stock_fde,
        mkinitfs_boot_deploy_native_unl0kr,
        mkinitfs_boot_deploy_start_hook,
        mkinitfs_boot_deploy_cleanup_hook,
        mkinitfs_boot_deploy_fde_call_snippet,
        openrc_supervisor_script,
        openrc_quit_script,
    };

    inline constexpr std::array template_ids{
        TemplateId::systemd_start_unit,
        TemplateId::systemd_show_unit,
        TemplateId::systemd_switch_root_unit,
        TemplateId::systemd_quit_unit,
        TemplateId::systemd_quit_wait_unit,
        TemplateId::systemd_console_agent_drop_in,
        TemplateId::dracut_systemd_config,
        TemplateId::dracut_systemd_module_setup,
        TemplateId::dracut_classic_module_setup,
        TemplateId::dracut_classic_start_hook,
        TemplateId::dracut_classic_askpass_patch_hook,
        TemplateId::dracut_classic_askpass_override,
        TemplateId::dracut_classic_pre_pivot_hook,
        TemplateId::mkinitcpio_install_hook,
        TemplateId::mkinitcpio_runtime_hook,
        TemplateId::mkinitcpio_plymouth_bridge,
        TemplateId::initramfs_tools_build_hook,
        TemplateId::initramfs_tools_askpass_wrapper,
        TemplateId::initramfs_tools_early_hook,
        TemplateId::initramfs_tools_bottom_hook,
        TemplateId::mkinitfs_feature_files,
        TemplateId::mkinitfs_runtime_hook,
        TemplateId::mkinitfs_findfs_wrapper,
        TemplateId::mkinitfs_early_call_snippet,
        TemplateId::mkinitfs_handoff_call_snippet,
        TemplateId::mkinitfs_boot_deploy_files,
        TemplateId::mkinitfs_boot_deploy_kernel_cmdline,
        TemplateId::mkinitfs_boot_deploy_apk_commit_hook,
        TemplateId::mkinitfs_boot_deploy_runtime,
        TemplateId::mkinitfs_boot_deploy_fde_wrapper,
        TemplateId::mkinitfs_boot_deploy_stock_fde,
        TemplateId::mkinitfs_boot_deploy_native_unl0kr,
        TemplateId::mkinitfs_boot_deploy_start_hook,
        TemplateId::mkinitfs_boot_deploy_cleanup_hook,
        TemplateId::mkinitfs_boot_deploy_fde_call_snippet,
        TemplateId::openrc_supervisor_script,
        TemplateId::openrc_quit_script,
    };

    enum class MaterializationKind : std::uint8_t {
        file,
        openrc_service,
        managed_snippet,
    };

    struct TemplateMaterialization {
        MaterializationKind kind;
        std::string_view path;
        std::uint16_t mode;
        std::string_view runlevel;
        std::string_view insertion_point;
    };

    struct TemplateResource {
        TemplateId id;
        TemplateMaterialization materialization;
        std::string_view contents;
        bool experimental_unproven;
    };

    std::string_view template_name(TemplateId id);
    TemplateResource template_resource(TemplateId id);

} // namespace sart::embedded

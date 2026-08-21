#pragma once

#include "sart/install/backends.hpp"

#include <optional>
#include <string>
#include <variant>

namespace sart::install {

    using ExactInstallContract =
        std::variant<DracutSystemdContract, InitramfsToolsSystemdContract, MkinitcpioSystemdContract,
                     MkinitfsOpenRcContract, MkinitfsBootDeployContract>;

    struct ExactInstallDiscovery {
        ExactInstallContract contract;
        AdapterId initramfs;
        AdapterId real_root;
    };

    ExactInstallDiscovery discover_exact_install_contract();
    std::optional<AndroidBootPartitionFact> installed_android_boot_partition();
    InstallPlan build_exact_self_install_plan(const ExactInstallDiscovery &discovery);
    std::string render_exact_install_plan(const InstallPlan &plan, const ExactInstallDiscovery &discovery, bool json);

} // namespace sart::install

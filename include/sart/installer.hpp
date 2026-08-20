#pragma once

#include "sart/adapter.hpp"

#include <cstddef>
#include <cstdint>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace sart::install {

    struct ExactInstallDiscovery;

    inline constexpr std::uint16_t plan_version = 3;
    inline constexpr std::uint64_t max_install_file_bytes = 64ULL * 1024 * 1024;
    inline constexpr std::uint64_t max_transaction_bytes = 128ULL * 1024 * 1024;

    enum class PlanSourceKind : std::uint8_t {
        sart_elf,
        embedded_template,
    };

    struct PlanSource {
        PlanSourceKind kind;
        embedded::TemplateId template_id{};
    };

    struct PlanOperation {
        std::string path;
        std::uint16_t mode;
        std::uint32_t owner_uid;
        std::string digest;
        PlanSource source;
        std::vector<std::byte> content;
    };

    struct ManagedSnippetOperation {
        AdapterId adapter;
        std::string target;
        std::string insertion_point;
        std::string digest;
        embedded::TemplateId source;
    };

    enum class ActivationScope : std::uint8_t {
        generated_initramfs,
        real_root,
    };

    enum class ActivationRelation : std::uint8_t {
        systemd_wants,
        openrc_runlevel,
    };

    struct ActivationOperation {
        AdapterId adapter;
        ActivationScope scope;
        ActivationRelation relation;
        std::string path;
        std::string relative_target;
        std::uint32_t owner_uid;
        embedded::TemplateId source;
        std::string runlevel;
    };

    struct InstallPlan {
        std::string root;
        AdapterId initramfs;
        AdapterId real_root;
        std::vector<PlanOperation> operations;
        std::vector<ManagedSnippetOperation> managed_snippets;
        std::vector<ActivationOperation> activations;

        [[nodiscard]] std::string identity() const;
    };

    void validate_static_elf(std::span<const std::byte> bytes);
    std::vector<std::byte> read_running_elf();
    InstallPlan build_install_plan(std::span<const std::byte> sart_elf, AdapterId initramfs, AdapterId real_root,
                                   bool allow_experimental = false, std::string root = "/");
    InstallPlan build_self_install_plan(AdapterId initramfs, AdapterId real_root, bool allow_experimental = false);

    std::string render_plan_human(const InstallPlan &plan, bool actionable);
    std::string render_plan_json(const InstallPlan &plan, bool actionable);

    enum class ApplyOutcome : std::uint8_t {
        installed,
        already_current,
        refreshed,
    };

    enum class RecoveryOutcome : std::uint8_t {
        no_transaction,
        rolled_back,
        completed_commit_cleaned,
    };

    enum class FileStatusState : std::uint8_t {
        exact,
        missing,
        mode_modified,
        content_modified,
        type_modified,
    };

    struct InstalledFileStatus {
        std::string path;
        std::uint16_t expected_mode;
        std::string expected_digest;
        FileStatusState state;
        std::string actual_digest;
    };

    struct StatusReport {
        bool installed;
        bool recovery_required;
        std::string transaction;
        std::string image_verification;
        std::vector<InstalledFileStatus> files;
    };

    struct UninstallReport {
        std::size_t removed;
        std::size_t restored;
        std::vector<std::string> preserved_directories;
    };

    class Installer {
      public:
        Installer(std::string root, std::uint32_t expected_owner_uid, bool mutation_unlocked);

        [[nodiscard]] const std::string &root() const noexcept;
        [[nodiscard]] StatusReport status() const;
        ApplyOutcome apply(const InstallPlan &plan);
        ApplyOutcome apply_exact(const InstallPlan &plan, const ExactInstallDiscovery &discovery);
        RecoveryOutcome recover();
        UninstallReport uninstall(const ExactInstallDiscovery *discovery = nullptr);

        static Installer live_root_read_only();
        static Installer live_root_mutating(std::string_view confirmed_hostname, bool package_hook = false);

      private:
        std::string root_;
        std::uint32_t expected_owner_uid_;
        bool mutation_unlocked_;
    };

    std::string render_status(const StatusReport &report);

} // namespace sart::install

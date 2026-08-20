#pragma once

#include "sart/installer.hpp"

#include <cstdint>
#include <expected>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace sart::install {

#if defined(__x86_64__)
    inline constexpr std::string_view product_architecture = "x86_64";
#elif defined(__aarch64__)
    inline constexpr std::string_view product_architecture = "aarch64";
#endif

    inline constexpr std::uint64_t max_candidate_bytes = 512ULL * 1024 * 1024;
    inline constexpr std::uint64_t min_boot_free_bytes = 3 * max_candidate_bytes;
    inline constexpr std::uint64_t min_boot_free_inodes = 16;
    inline constexpr std::size_t max_archive_entries = 262144;
    inline constexpr std::uint64_t max_inspected_archive_bytes = 768ULL * 1024 * 1024;

    enum class GeneratorKind : std::uint8_t {
        dracut,
        initramfs_inspection,
        grub_update,
        extlinux_update,
        initramfs_tools,
        mkinitcpio,
        mkinitfs,
        mkinitfs_boot_deploy,
        systemd_reload,
        openrc_runlevel,
    };

    struct GeneratorRequest {
        GeneratorKind generator;
        std::string executable;
        std::string alternate_root;
        std::optional<std::string> working_directory;
        std::vector<std::string> arguments;
        bool clear_environment{true};
    };

    struct CommandOutput {
        int status;
        std::vector<std::byte> standard_output;
        std::vector<std::byte> standard_error;
    };

    inline constexpr std::size_t max_generator_output_bytes = 1024 * 1024;

    std::string_view generator_kind_name(GeneratorKind kind);
    void validate_supported_generator_request(const GeneratorRequest &request);
    CommandOutput run_generator(const GeneratorRequest &request);

    struct ToolFact {
        std::string path;
        bool root_owned;
        bool regular;
        bool symlink;
        bool executable;

        static ToolFact exact(std::string_view path);
    };

    enum class CryptsetupLocation : std::uint8_t { usr_bin, usr_sbin };
    std::string_view cryptsetup_executable(CryptsetupLocation location);

    enum class GrubRegeneration : std::uint8_t { update_grub, grub2_mkconfig, grub_mkconfig };
    std::string_view grub_updater(GrubRegeneration regeneration);
    std::string_view grub_probe(GrubRegeneration regeneration);
    std::string_view grub_config_path(GrubRegeneration regeneration);
    std::vector<std::string> grub_arguments(GrubRegeneration regeneration);

    std::vector<std::byte> render_grub_script(std::string_view boot_uuid, std::string_view kernel,
                                              std::string_view command_line, std::string_view known_good_image);

    enum class ArchiveEntryKind : std::uint8_t { file, directory, symlink, character_device };

    struct ArchiveEntry {
        std::string path;
        ArchiveEntryKind kind;
        std::uint16_t mode;
        std::vector<std::byte> bytes;
        std::uint32_t device_major{};
        std::uint32_t device_minor{};
    };

    struct ArchiveInspection {
        std::string sart_digest;
        std::size_t inspected_entries;
        std::uint64_t inspected_bytes;
    };

    struct SartFreeArchiveInspection {
        std::size_t inspected_entries;
        std::uint64_t inspected_bytes;
    };

    enum class DracutImageLayout : std::uint8_t { initrd_img, initramfs_img };

    struct DracutSystemdFacts {
        std::string architecture;
        std::string pid1_comm;
        std::vector<std::string> kernel_versions;
        std::uint64_t root_filesystem_device;
        std::uint64_t boot_filesystem_device;
        bool boot_writable;
        std::uint64_t boot_free_bytes;
        std::uint64_t boot_free_inodes;
        std::vector<std::string> dracut_modules;
        DracutImageLayout image_layout;
        GrubRegeneration grub_regeneration;
        CryptsetupLocation cryptsetup_location;
        std::vector<ToolFact> tools;
        std::string known_good_path;
        std::string known_good_digest;
        std::uint64_t known_good_bytes;
        std::string boot_filesystem_uuid;
        std::string kernel_command_line;
    };

    struct DracutSystemdContract {
        DracutImageLayout image_layout;
        GrubRegeneration grub_regeneration;
        std::string kernel_version;
        std::string active_image;
        std::string candidate_image;
        std::string known_good_image;
        std::string known_good_digest;
        std::string grub_script_path;
        std::string grub_config_path;
        std::vector<std::byte> grub_script;
        GeneratorRequest generate;
        GeneratorRequest update_grub;
    };

    struct DracutSystemdImageRecord {
        std::string kernel_version;
        std::string active_image;
        std::string active_digest;
        std::string candidate_image;
        std::string candidate_digest;
        std::uint64_t candidate_bytes;
        std::string known_good_image;
        std::string known_good_digest;
        std::string grub_script_path;
        std::string grub_script_digest;
        std::string grub_config_path;
        std::string sart_digest;
    };

    DracutSystemdContract plan_dracut_systemd(const DracutSystemdFacts &facts, std::string alternate_root = "/");
    void validate_dracut_systemd_contract(const DracutSystemdContract &contract);
    void validate_dracut_systemd_generator_request(const GeneratorRequest &request);
    GeneratorRequest dracut_systemd_unpack_request(const DracutSystemdContract &contract, std::string_view transaction);
    bool dracut_systemd_managed_image_path(std::string_view path);
    ArchiveInspection inspect_dracut_inventory(const std::vector<ArchiveEntry> &entries,
                                               std::span<const std::byte> expected_sart);
    SartFreeArchiveInspection inspect_sart_free_dracut_inventory(const std::vector<ArchiveEntry> &entries);
    DracutSystemdImageRecord verified_dracut_systemd_image_record(const DracutSystemdContract &contract,
                                                                  std::span<const std::byte> candidate,
                                                                  const ArchiveInspection &inspection,
                                                                  std::span<const std::byte> expected_sart);
    void validate_dracut_systemd_image_record(const DracutSystemdImageRecord &record);
    GeneratorRequest dracut_systemd_sart_free_generate_request(const DracutSystemdImageRecord &record,
                                                               std::string alternate_root = "/");
    GeneratorRequest dracut_systemd_sart_free_unpack_request(const DracutSystemdImageRecord &record,
                                                             std::string_view transaction,
                                                             std::string alternate_root = "/");
    std::vector<ArchiveEntry> collect_unpacked_archive_inventory(std::string_view unpacked_root,
                                                                 std::uint32_t expected_owner_uid);

    struct InitramfsToolsPathFact {
        std::string path;
        bool root_owned;
        bool regular;
        bool symlink;
        bool executable;
    };

    struct InitramfsToolsSystemdFacts {
        std::string architecture;
        std::string pid1_comm;
        std::vector<std::string> kernel_versions;
        std::uint64_t root_filesystem_device;
        std::uint64_t boot_filesystem_device;
        bool boot_writable;
        std::uint64_t boot_free_bytes;
        std::uint64_t boot_free_inodes;
        GrubRegeneration grub_regeneration;
        CryptsetupLocation cryptsetup_location;
        std::vector<ToolFact> tools;
        std::vector<InitramfsToolsPathFact> contract_files;
        std::string known_good_path;
        std::string known_good_digest;
        std::uint64_t known_good_bytes;
        std::string boot_filesystem_uuid;
        std::string kernel_command_line;
    };

    struct InitramfsToolsSystemdContract {
        std::string kernel_version;
        std::string active_image;
        std::string candidate_image;
        std::string known_good_image;
        std::string known_good_digest;
        GrubRegeneration grub_regeneration;
        std::string grub_script_path;
        std::string grub_config_path;
        std::vector<std::byte> grub_script;
        GeneratorRequest generate;
        GeneratorRequest update_grub;
    };

    InitramfsToolsSystemdContract plan_initramfs_tools_systemd(const InitramfsToolsSystemdFacts &facts,
                                                               std::string alternate_root = "/");
    void validate_initramfs_tools_systemd_contract(const InitramfsToolsSystemdContract &contract);
    void validate_initramfs_tools_systemd_generator_request(const GeneratorRequest &request);
    GeneratorRequest initramfs_tools_systemd_unpack_request(const InitramfsToolsSystemdContract &contract,
                                                            std::string_view transaction);
    bool initramfs_tools_systemd_managed_image_path(std::string_view path);
    std::vector<ArchiveEntry> collect_unpacked_initramfs_tools_inventory(std::string_view unpacked_root,
                                                                         std::uint32_t expected_owner_uid);
    ArchiveInspection inspect_initramfs_tools_inventory(const std::vector<ArchiveEntry> &entries,
                                                        std::span<const std::byte> expected_sart);
    SartFreeArchiveInspection inspect_sart_free_initramfs_tools_inventory(const std::vector<ArchiveEntry> &entries);
    DracutSystemdImageRecord verified_initramfs_tools_systemd_image_record(
        const InitramfsToolsSystemdContract &contract, std::span<const std::byte> candidate,
        const ArchiveInspection &inspection, std::span<const std::byte> expected_sart);

    struct MkinitfsOpenRcPathFact {
        std::string path;
        bool root_owned;
        bool regular;
        bool symlink;
        bool executable;
        std::uint16_t mode;
        std::string digest;
    };

    struct MkinitfsOpenRcFacts {
        std::string architecture;
        std::string pid1_comm;
        std::vector<std::string> kernel_versions;
        bool boot_writable;
        std::uint64_t boot_free_bytes;
        std::uint64_t boot_free_inodes;
        std::vector<ToolFact> tools;
        std::vector<MkinitfsOpenRcPathFact> contract_files;
        std::string initramfs_init_source;
        std::string mkinitfs_config_source;
        std::vector<std::string> mkinitfs_features;
        bool extlinux_overwrite;
        std::string extlinux_default_label;
        std::string kernel_command_line;
        std::string known_good_path;
        std::string known_good_digest;
        std::uint64_t known_good_bytes;
    };

    struct MkinitfsOpenRcContract {
        std::string kernel_version;
        std::string kernel_flavor;
        std::string kernel_image;
        std::string active_image;
        std::string candidate_image;
        std::string known_good_image;
        std::string known_good_digest;
        std::string extlinux_fragment_path;
        std::string extlinux_config_path;
        std::vector<std::byte> extlinux_fragment;
        std::string mkinitfs_config_path;
        std::uint16_t mkinitfs_config_mode;
        std::vector<std::byte> mkinitfs_config_original;
        std::vector<std::byte> mkinitfs_config_activated;
        bool mkinitfs_config_already_active;
        std::vector<MkinitfsOpenRcPathFact> prerequisites;
        GeneratorRequest generate;
        GeneratorRequest update_extlinux;
    };

    struct ExtlinuxSettings {
        bool overwrite;
        std::string default_label;
        std::string kernel_command_line;
    };

    std::vector<std::string> parse_mkinitfs_features(std::string_view source);
    std::vector<std::byte> activate_mkinitfs_sart_feature(std::string_view source);
    ExtlinuxSettings parse_update_extlinux_settings(std::string_view source);
    std::string parse_extlinux_entry_command_line(std::string_view source, std::string_view label);
    MkinitfsOpenRcContract plan_mkinitfs_openrc(const MkinitfsOpenRcFacts &facts, std::string alternate_root = "/");
    void validate_mkinitfs_openrc_contract(const MkinitfsOpenRcContract &contract);
    void validate_mkinitfs_openrc_generator_request(const GeneratorRequest &request);
    ArchiveInspection inspect_mkinitfs_openrc_archive(std::span<const std::byte> candidate,
                                                      std::span<const std::byte> expected_sart);
    DracutSystemdImageRecord verified_mkinitfs_openrc_image_record(const MkinitfsOpenRcContract &contract,
                                                                   std::span<const std::byte> candidate,
                                                                   const ArchiveInspection &inspection,
                                                                   std::span<const std::byte> expected_sart);
    void validate_mkinitfs_openrc_image_record(const DracutSystemdImageRecord &record);
    bool mkinitfs_openrc_managed_image_path(std::string_view path);

    struct AndroidBootDeviceInfo {
        std::string architecture;
        std::string codename;
        std::string flash_method;
        std::string boot_partition_label;
        std::string dtb;
        std::uint32_t header_version;
        std::vector<std::byte> no_flash_deviceinfo;
    };

    struct AndroidBootPartitionFact {
        std::string label;
        std::string canonical_path;
        std::uint64_t device_number;
        std::uint64_t bytes;
        std::string digest;
    };

    struct AndroidBootFacts {
        AndroidBootDeviceInfo deviceinfo;
        AndroidBootPartitionFact partition;
        std::string dtb_path;
        std::string dtb_digest;
        std::uint64_t dtb_bytes;
    };

    struct AndroidBootImageInspection {
        std::uint32_t page_size;
        std::uint32_t kernel_size;
        std::uint32_t ramdisk_size;
        std::uint32_t dtb_size;
        std::string image_digest;
    };

    std::string android_slot_partition_label(std::string_view kernel_command_line, std::string_view base_label);
    std::optional<AndroidBootDeviceInfo>
    parse_android_boot_deviceinfo(std::string_view vendor_source,
                                  std::optional<std::string_view> override_source = std::nullopt,
                                  bool accept_existing_sart_guard = false);
    bool deviceinfo_enables_kernel_flash(std::string_view source);
    bool deviceinfo_generates_android_boot_image(std::string_view source);
    AndroidBootImageInspection inspect_android_boot_image_v2(std::span<const std::byte> image,
                                                             std::span<const std::byte> expected_kernel,
                                                             std::span<const std::byte> expected_ramdisk,
                                                             std::span<const std::byte> expected_dtb);
    std::string activate_android_boot_partition(std::vector<std::byte> &partition, std::span<const std::byte> original,
                                                std::span<const std::byte> candidate_boot_image);
    void restore_android_boot_partition(std::vector<std::byte> &partition, std::span<const std::byte> original);
    bool safe_android_partition_fact(const AndroidBootPartitionFact &partition);
    AndroidBootPartitionFact inspect_android_boot_partition(std::string_view label);
    std::vector<std::byte> read_android_boot_partition(const AndroidBootPartitionFact &partition,
                                                       bool require_discovery_digest = true);
    std::string activate_android_boot_partition_durable(const AndroidBootPartitionFact &partition,
                                                        std::span<const std::byte> original,
                                                        std::span<const std::byte> candidate_boot_image);
    void restore_android_boot_partition_durable(const AndroidBootPartitionFact &partition,
                                                std::span<const std::byte> original);

    enum class MkinitfsBootDeployCompression : std::uint8_t { gzip, zstandard };

    MkinitfsBootDeployCompression detect_mkinitfs_boot_deploy_compression(std::span<const std::byte> image);
    std::vector<std::byte> decompress_mkinitfs_boot_deploy_archive(std::span<const std::byte> candidate,
                                                                   MkinitfsBootDeployCompression expected);
    std::uint64_t mkinitfs_boot_deploy_initial_boot_bytes(std::uint64_t kernel_bytes,
                                                          std::uint64_t active_initramfs_bytes,
                                                          std::uint64_t allocation_unit);
    std::uint64_t mkinitfs_boot_deploy_preservation_bytes(std::uint64_t active_initramfs_bytes,
                                                          std::uint64_t known_good_entry_bytes,
                                                          std::uint64_t allocation_unit);

    struct MkinitfsBootDeployPathFact {
        std::string path;
        bool root_owned;
        bool regular;
        bool symlink;
        bool executable;
    };

    struct MkinitfsBootDeployFacts {
        std::string architecture;
        std::string pid1_comm;
        std::uint64_t root_filesystem_device;
        std::uint64_t boot_filesystem_device;
        bool boot_writable;
        std::uint64_t boot_free_bytes;
        std::uint64_t boot_allocation_unit;
        std::uint64_t boot_total_inodes;
        std::uint64_t boot_free_inodes;
        std::vector<ToolFact> tools;
        std::vector<MkinitfsBootDeployPathFact> contract_files;
        std::string initramfs_version;
        std::string init_functions_2nd;
        std::string kernel_image;
        std::uint64_t kernel_bytes;
        std::string active_image;
        MkinitfsBootDeployCompression active_image_compression;
        std::string known_good_digest;
        std::uint64_t known_good_bytes;
        std::string active_loader_entry;
        std::uint16_t active_loader_entry_mode;
        std::vector<std::byte> active_loader_entry_bytes;
        std::string kernel_command_line;
        std::optional<AndroidBootFacts> android_boot;
    };

    struct AndroidBootGenerationContract {
        AndroidBootDeviceInfo deviceinfo;
        AndroidBootPartitionFact partition;
        std::string dtb_path;
        std::string dtb_digest;
        std::uint64_t dtb_bytes;
        std::string deviceinfo_path;
        std::string candidate_boot_image;
    };

    struct MkinitfsBootDeployContract {
        std::string kernel_image;
        std::uint64_t kernel_bytes;
        std::string active_image;
        std::uint64_t active_image_bytes;
        MkinitfsBootDeployCompression expected_compression;
        std::uint64_t boot_filesystem_device;
        std::uint64_t boot_allocation_unit;
        std::string candidate_directory;
        std::string candidate_image;
        std::string candidate_kernel;
        std::string known_good_image;
        std::string known_good_digest;
        std::string known_good_entry_path;
        std::uint16_t known_good_entry_mode;
        std::vector<std::byte> known_good_entry;
        std::string active_loader_entry;
        std::uint16_t active_loader_entry_mode;
        std::vector<std::byte> active_loader_entry_original;
        std::vector<std::byte> active_loader_entry_activated;
        std::vector<std::byte> patched_init_functions_2nd;
        GeneratorRequest generate;
        std::optional<AndroidBootGenerationContract> android_boot;
    };

    std::pair<std::string, std::string> parse_mkinitfs_boot_deploy_loader_entry(std::string_view source);
    std::vector<std::byte> activate_mkinitfs_boot_deploy_loader_entry(std::string_view source);
    std::string parse_mkinitfs_boot_deploy_version(std::string_view source);
    MkinitfsBootDeployContract plan_mkinitfs_boot_deploy(const MkinitfsBootDeployFacts &facts, bool systemd_real_root,
                                                         std::string alternate_root = "/");
    void validate_mkinitfs_boot_deploy_contract(const MkinitfsBootDeployContract &contract);
    void validate_mkinitfs_boot_deploy_generator_request(const GeneratorRequest &request);
    ArchiveInspection inspect_mkinitfs_boot_deploy_archive(std::span<const std::byte> candidate,
                                                           std::span<const std::byte> expected_sart);
    SartFreeArchiveInspection inspect_sart_free_mkinitfs_boot_deploy_archive(std::span<const std::byte> candidate);
    DracutSystemdImageRecord verified_mkinitfs_boot_deploy_image_record(const MkinitfsBootDeployContract &contract,
                                                                        std::span<const std::byte> candidate,
                                                                        const ArchiveInspection &inspection,
                                                                        std::span<const std::byte> expected_sart);
    void validate_mkinitfs_boot_deploy_image_record(const DracutSystemdImageRecord &record);
    bool mkinitfs_boot_deploy_managed_image_path(std::string_view path);

    struct MkinitcpioPathFact {
        std::string path;
        bool root_owned;
        bool regular;
        bool symlink;
        bool executable;
    };

    struct MkinitcpioSystemdFacts {
        std::string architecture;
        std::string pid1_comm;
        std::vector<std::string> kernel_versions;
        std::string package_base;
        std::uint64_t root_filesystem_device;
        std::uint64_t boot_filesystem_device;
        bool boot_writable;
        std::uint64_t boot_free_bytes;
        std::uint64_t boot_free_inodes;
        CryptsetupLocation cryptsetup_location;
        std::vector<ToolFact> tools;
        std::vector<MkinitcpioPathFact> contract_files;
        std::string config_source;
        std::uint16_t config_mode;
        std::string preset_source;
        std::string known_good_path;
        std::string known_good_digest;
        std::uint64_t known_good_bytes;
        std::string boot_filesystem_uuid;
        std::string kernel_command_line;
    };

    struct MkinitcpioSystemdContract {
        std::string kernel_version;
        std::string package_base;
        std::string preset_path;
        std::string active_image;
        std::string candidate_image;
        std::string known_good_image;
        std::string known_good_digest;
        std::string config_path;
        std::uint16_t config_mode;
        std::vector<std::byte> config_original;
        std::vector<std::byte> config_activated;
        bool config_already_active;
        GrubRegeneration grub_regeneration;
        std::string grub_script_path;
        std::string grub_config_path;
        std::vector<std::byte> grub_script;
        GeneratorRequest generate;
        GeneratorRequest update_grub;
    };

    std::expected<std::string, std::string> activate_mkinitcpio_hooks(std::string_view source);
    MkinitcpioSystemdContract plan_mkinitcpio_systemd(const MkinitcpioSystemdFacts &facts,
                                                      std::string alternate_root = "/");
    void validate_mkinitcpio_systemd_contract(const MkinitcpioSystemdContract &contract);
    void validate_mkinitcpio_systemd_generator_request(const GeneratorRequest &request);
    GeneratorRequest mkinitcpio_unpack_request(const MkinitcpioSystemdContract &contract, std::string_view transaction);
    ArchiveInspection inspect_mkinitcpio_inventory(const std::vector<ArchiveEntry> &entries,
                                                   std::span<const std::byte> expected_sart);
    DracutSystemdImageRecord verified_mkinitcpio_systemd_image_record(const MkinitcpioSystemdContract &contract,
                                                                      std::span<const std::byte> candidate,
                                                                      const ArchiveInspection &inspection,
                                                                      std::span<const std::byte> expected_sart);

} // namespace sart::install

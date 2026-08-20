#include "sart/installer.hpp"

#include "sart/sha256.hpp"

#include <algorithm>
#include <array>
#include <format>
#include <set>
#include <stdexcept>

namespace sart::install {
    namespace {

        std::span<const std::byte> as_bytes(std::string_view value) {
            return std::as_bytes(std::span(value.data(), value.size()));
        }

        void push_field(std::vector<std::byte> &output, std::string_view value) {
            const auto length = static_cast<std::uint64_t>(value.size());
            for (int shift = 56; shift >= 0; shift -= 8) {
                output.push_back(std::byte(static_cast<unsigned char>(length >> shift)));
            }
            const auto bytes = as_bytes(value);
            output.insert(output.end(), bytes.begin(), bytes.end());
        }

        std::string source_name(const PlanSource &source) {
            if (source.kind == PlanSourceKind::sart_elf) {
                return "sart.elf";
            }
            return std::string(embedded::template_name(source.template_id));
        }

        void add_activation(std::vector<ActivationOperation> &output, ActivationOperation operation) {
            output.push_back(std::move(operation));
        }

        std::string json_escape(std::string_view input) {
            std::string output;
            for (const unsigned char byte : input) {
                switch (byte) {
                case '"':
                    output += "\\\"";
                    break;
                case '\\':
                    output += "\\\\";
                    break;
                case '\b':
                    output += "\\b";
                    break;
                case '\f':
                    output += "\\f";
                    break;
                case '\n':
                    output += "\\n";
                    break;
                case '\r':
                    output += "\\r";
                    break;
                case '\t':
                    output += "\\t";
                    break;
                default:
                    if (byte < 0x20) {
                        output += std::format("\\u{:04x}", byte);
                    } else {
                        output.push_back(static_cast<char>(byte));
                    }
                }
            }
            return output;
        }

    } // namespace

    std::string InstallPlan::identity() const {
        std::vector<std::byte> bytes;
        push_field(bytes, "sart.install-plan.identity");
        bytes.push_back(std::byte{0});
        bytes.push_back(std::byte{static_cast<unsigned char>(plan_version)});
        bytes.push_back(std::byte{0});
        bytes.push_back(std::byte{static_cast<unsigned char>(embedded::resource_set_version)});
        push_field(bytes, root);
        push_field(bytes, adapter_name(initramfs));
        push_field(bytes, adapter_name(real_root));
        for (const auto &operation : operations) {
            push_field(bytes, operation.path);
            push_field(bytes, operation.digest);
            push_field(bytes, source_name(operation.source));
        }
        for (const auto &operation : managed_snippets) {
            push_field(bytes, operation.target);
            push_field(bytes, operation.insertion_point);
            push_field(bytes, operation.digest);
        }
        for (const auto &operation : activations) {
            push_field(bytes, operation.path);
            push_field(bytes, operation.relative_target);
        }
        return sha256(bytes);
    }

    InstallPlan build_install_plan(std::span<const std::byte> sart_elf, AdapterId initramfs, AdapterId real_root,
                                   bool allow_experimental, std::string root) {
        validate_static_elf(sart_elf);
        if (root.empty() || root.front() != '/' || root.contains("/../") || root.ends_with("/..")) {
            throw std::runtime_error("installer root is not a safe absolute path");
        }
        const auto *pair = adapter_pair(initramfs, real_root);
        if (pair == nullptr) {
            throw std::runtime_error("adapter pair is not an explicit supported combination");
        }
        if (pair->status != SupportStatus::proven_supported && !allow_experimental) {
            throw std::runtime_error(std::string(pair->limitation));
        }
        if (adapter_metadata(initramfs).kind != AdapterKind::initramfs_runtime ||
            adapter_metadata(real_root).kind != AdapterKind::real_root_supervisor) {
            throw std::runtime_error("adapter kind mismatch");
        }

        InstallPlan plan{std::move(root), initramfs, real_root, {}, {}, {}};
        plan.operations.push_back({
            "/usr/bin/sart",
            0755,
            0,
            sha256(sart_elf),
            {PlanSourceKind::sart_elf, {}},
            {sart_elf.begin(), sart_elf.end()},
        });
        std::set<std::string> paths{"/usr/bin/sart"};
        for (const auto adapter : {initramfs, real_root}) {
            for (const auto id : adapter_metadata(adapter).resources) {
                const auto resource = embedded::template_resource(id);
                if (resource.materialization.kind == embedded::MaterializationKind::managed_snippet) {
                    plan.managed_snippets.push_back({
                        adapter,
                        std::string(resource.materialization.path),
                        std::string(resource.materialization.insertion_point),
                        sha256(resource.contents),
                        id,
                    });
                    continue;
                }
                if (!paths.insert(std::string(resource.materialization.path)).second) {
                    throw std::runtime_error("duplicate installer destination");
                }
                const auto contents = as_bytes(resource.contents);
                plan.operations.push_back({
                    std::string(resource.materialization.path),
                    resource.materialization.mode,
                    0,
                    sha256(contents),
                    {PlanSourceKind::embedded_template, id},
                    {contents.begin(), contents.end()},
                });
            }
        }
        std::ranges::sort(plan.operations, {}, &PlanOperation::path);
        std::ranges::sort(plan.managed_snippets, {}, &ManagedSnippetOperation::target);
        std::uint64_t total = 0;
        for (const auto &operation : plan.operations) {
            if (operation.content.size() > max_transaction_bytes - total) {
                throw std::runtime_error("installer payload exceeds the transaction size limit");
            }
            total += operation.content.size();
        }

        if (initramfs == AdapterId::dracut_systemd) {
            add_activation(plan.activations, {initramfs,
                                              ActivationScope::generated_initramfs,
                                              ActivationRelation::systemd_wants,
                                              "/usr/lib/systemd/system/initrd.target.wants/sart-start.service",
                                              "../sart-start.service",
                                              0,
                                              embedded::TemplateId::systemd_start_unit,
                                              {}});
            add_activation(plan.activations, {initramfs,
                                              ActivationScope::generated_initramfs,
                                              ActivationRelation::systemd_wants,
                                              "/usr/lib/systemd/system/initrd.target.wants/sart-show.service",
                                              "../sart-show.service",
                                              0,
                                              embedded::TemplateId::systemd_show_unit,
                                              {}});
            add_activation(plan.activations,
                           {initramfs,
                            ActivationScope::generated_initramfs,
                            ActivationRelation::systemd_wants,
                            "/usr/lib/systemd/system/initrd-switch-root.target.wants/sart-switch-root.service",
                            "../sart-switch-root.service",
                            0,
                            embedded::TemplateId::systemd_switch_root_unit,
                            {}});
        }
        if (real_root == AdapterId::systemd_real_root) {
            add_activation(plan.activations, {real_root,
                                              ActivationScope::real_root,
                                              ActivationRelation::systemd_wants,
                                              "/etc/systemd/system/multi-user.target.wants/sart-quit.service",
                                              "../../../../usr/lib/systemd/system/sart-quit.service",
                                              0,
                                              embedded::TemplateId::systemd_quit_unit,
                                              {}});
        } else if (real_root == AdapterId::openrc_real_root) {
            add_activation(plan.activations,
                           {real_root, ActivationScope::real_root, ActivationRelation::openrc_runlevel,
                            "/etc/runlevels/boot/sart", "../../init.d/sart", 0,
                            embedded::TemplateId::openrc_supervisor_script, "boot"});
            add_activation(plan.activations,
                           {real_root, ActivationScope::real_root, ActivationRelation::openrc_runlevel,
                            "/etc/runlevels/default/sart-quit", "../../init.d/sart-quit", 0,
                            embedded::TemplateId::openrc_quit_script, "default"});
        }
        std::ranges::sort(plan.activations, {}, &ActivationOperation::path);
        return plan;
    }

    InstallPlan build_self_install_plan(AdapterId initramfs, AdapterId real_root, bool allow_experimental) {
        const auto elf = read_running_elf();
        return build_install_plan(elf, initramfs, real_root, allow_experimental);
    }

    std::string render_plan_human(const InstallPlan &plan, bool actionable) {
        std::string output = std::format("sart install plan v{}\nstatus: {}\nmutation: {}\nresource-set: "
                                         "{}\nplan-id: {}\nroot: {}\nadapters: {} + {}\noperations:\n",
                                         plan_version, actionable ? "READY" : "BLOCKED",
                                         actionable ? "GUARDED (uid-0 + exact-hostname + interactive-tty)" : "LOCKED",
                                         embedded::resource_set_version, plan.identity(), plan.root,
                                         adapter_name(plan.initramfs), adapter_name(plan.real_root));
        std::size_t index = 1;
        for (const auto &operation : plan.operations) {
            output += std::format("  {:03} write {} mode={:04o} owner={} sha256={} source={} previous=uninspected\n",
                                  index++, operation.path, operation.mode, operation.owner_uid, operation.digest,
                                  source_name(operation.source));
        }
        for (const auto &operation : plan.managed_snippets) {
            output +=
                std::format("  {:03} patch {} point={} sha256={} source={}\n", index++, operation.target,
                            operation.insertion_point, operation.digest, embedded::template_name(operation.source));
        }
        for (const auto &operation : plan.activations) {
            output += std::format("  {:03} symlink {} -> {} scope={} owner={}\n", index++, operation.path,
                                  operation.relative_target,
                                  operation.scope == ActivationScope::real_root ? "real_root" : "generated_initramfs",
                                  operation.owner_uid);
        }
        output +=
            "transaction: durable-journal -> preimages -> candidate-generate -> bounded-inspect -> atomic-activate -> "
            "manifest-commit\nrollback: exact preimages + explicit recover\nnetwork: forbidden\n";
        return output;
    }

    std::string render_plan_json(const InstallPlan &plan, bool actionable) {
        std::string operations;
        for (const auto &operation : plan.operations) {
            if (!operations.empty())
                operations += ',';
            operations += std::format("{{\"kind\":\"write_file\",\"path\":\"{}\",\"mode\":{},\"owner_uid\":{},"
                                      "\"sha256\":\"{}\",\"source\":\"{}\",\"previous\":\"uninspected\"}}",
                                      json_escape(operation.path), operation.mode, operation.owner_uid,
                                      operation.digest, json_escape(source_name(operation.source)));
        }
        return std::format("{{\"schema\":\"sart.install-plan\",\"version\":{},\"resource_set_version\":{},\"plan_"
                           "id\":\"{}\",\"actionable\":{},\"mutation\":\"{}\",\"root\":\"{}\",\"adapters\":[\"{}\",\"{}"
                           "\"],\"operations\":[{}],\"network\":\"forbidden\"}}",
                           plan_version, embedded::resource_set_version, plan.identity(), actionable ? "true" : "false",
                           actionable ? "guarded" : "locked", json_escape(plan.root),
                           json_escape(adapter_name(plan.initramfs)), json_escape(adapter_name(plan.real_root)),
                           operations);
    }

} // namespace sart::install

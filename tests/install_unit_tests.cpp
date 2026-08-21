#include "sart/embedded/resources.hpp"
#include "sart/install/adapter.hpp"
#include "sart/integration/patch.hpp"

#include <doctest/doctest.h>

#include <set>
#include <stdexcept>
#include <string>

TEST_SUITE("install") {

    TEST_CASE("adapter catalog has unique identifiers and names") {
        std::set<sart::install::AdapterId> identifiers;
        std::set<std::string_view> names;
        for (const auto identifier : sart::install::adapter_ids) {
            const auto &metadata = sart::install::adapter_metadata(identifier);
            CHECK(metadata.id == identifier);
            CHECK_FALSE(metadata.name.empty());
            CHECK(identifiers.insert(identifier).second);
            CHECK(names.insert(metadata.name).second);
        }
        CHECK(identifiers.size() == sart::install::adapter_ids.size());
    }

    TEST_CASE("adapter resources resolve to matching embedded records") {
        for (const auto identifier : sart::install::adapter_ids) {
            for (const auto template_id : sart::install::adapter_metadata(identifier).resources) {
                const auto resource = sart::embedded::template_resource(template_id);
                CHECK(resource.id == template_id);
                CHECK_FALSE(sart::embedded::template_name(template_id).empty());
                CHECK_FALSE(resource.materialization.path.empty());
                CHECK_FALSE(resource.contents.empty());
            }
        }
    }

    TEST_CASE("unknown adapter identifier is rejected") {
        CHECK_THROWS_AS(sart::install::adapter_metadata(static_cast<sart::install::AdapterId>(255)),
                        std::invalid_argument);
    }

    TEST_CASE("adapter pair catalog has unique proof slugs") {
        std::set<std::string_view> slugs;
        for (const auto &pair : sart::install::adapter_pairs()) {
            CHECK_FALSE(pair.proof_slug.empty());
            CHECK(slugs.insert(pair.proof_slug).second);
            CHECK(pair.proof_gates.size() == 6);
            CHECK_FALSE(pair.limitation.empty());
        }
    }

    TEST_CASE("each adapter pair can be looked up exactly") {
        for (const auto &pair : sart::install::adapter_pairs()) {
            const auto *found = sart::install::adapter_pair(pair.initramfs, pair.real_root);
            REQUIRE(found != nullptr);
            CHECK(found->proof_slug == pair.proof_slug);
        }
    }

    TEST_CASE("unregistered adapter pair returns null") {
        CHECK(sart::install::adapter_pair(sart::install::AdapterId::dracut_systemd,
                                          sart::install::AdapterId::openrc_real_root) == nullptr);
    }

    TEST_CASE("proof gates retain Make entrypoints") {
        for (const auto &pair : sart::install::adapter_pairs()) {
            for (const auto gate : pair.proof_gates) {
                CHECK(gate.starts_with("make vm-test-"));
            }
        }
    }

    TEST_CASE("embedded template catalog has unique names") {
        std::set<std::string_view> names;
        for (const auto identifier : sart::embedded::template_ids) {
            CHECK(names.insert(sart::embedded::template_name(identifier)).second);
        }
        CHECK(names.size() == sart::embedded::template_ids.size());
    }

    TEST_CASE("embedded file modes are data or executable") {
        for (const auto identifier : sart::embedded::template_ids) {
            const auto resource = sart::embedded::template_resource(identifier);
            if (resource.materialization.kind == sart::embedded::MaterializationKind::managed_snippet) {
                CHECK(resource.materialization.mode == 0);
                CHECK_FALSE(resource.materialization.insertion_point.empty());
            } else {
                CHECK((resource.materialization.mode == 0644 || resource.materialization.mode == 0755));
            }
        }
    }

    TEST_CASE("patchers reject unsupported input contracts") {
        const auto mkinitfs = sart::integration::patch_mkinitfs_init("");
        REQUIRE_FALSE(mkinitfs.has_value());
        CHECK(mkinitfs.error() == sart::integration::PatchError::unsupported_version);

        const auto boot_deploy = sart::integration::patch_boot_deploy_init_functions(
            "", sart::integration::reviewed_boot_deploy_initramfs_version);
        REQUIRE_FALSE(boot_deploy.has_value());
        CHECK(boot_deploy.error() == sart::integration::PatchError::ambiguous_unlock_function);
    }

    TEST_CASE("boot-deploy patcher rejects unreviewed versions first") {
        const auto result = sart::integration::patch_boot_deploy_init_functions("anything", "0.0.0");
        REQUIRE_FALSE(result.has_value());
        CHECK(result.error() == sart::integration::PatchError::unsupported_version);
    }

} // TEST_SUITE

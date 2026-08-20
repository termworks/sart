#!/usr/bin/env bash
# TEST INFRASTRUCTURE ONLY. Fetch upstream sources transitively pinned by pmaports.

set -Eeuo pipefail
umask 077
ulimit -c 0
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
source "$SCRIPT_DIR/lib.sh"

[[ $# -eq 3 ]] || vm_die \
    'usage: review-postmarketos-components.sh REPO_ROOT VM_ROOT SOURCE_LOCK'
repo_root=$1
vm_root=$2
source_lock=$3

vm_refuse_root
vm_validate_state "$repo_root" "$vm_root"
vm_validate_postmarketos_source_lock "$source_lock"

pmaports_commit=
pmaports_sha=
pmaports_bytes=
while IFS='|' read -r component commit url sha download_bytes; do
    [[ -z "$component" || "$component" == \#* ]] && continue
    if [[ "$component" == pmaports ]]; then
        pmaports_commit=$commit
        pmaports_sha=$sha
        pmaports_bytes=$download_bytes
        break
    fi
done < "$source_lock"
[[ "$pmaports_commit" =~ ^[0-9a-f]{40}$ && "$pmaports_sha" =~ ^[0-9a-f]{64}$ ]] ||
    vm_die 'cannot resolve the pinned pmaports source row'

pmaports="$vm_root/cache/postmarketos-sources/pmaports-$pmaports_commit.tar.gz"
[[ -f "$pmaports" && ! -L "$pmaports" && "$(vm_stat_mode "$pmaports")" == 400 ]] ||
    vm_die 'pinned pmaports archive is not present and sealed'
vm_assert_owned "$pmaports"
vm_assert_file_size_exact "$pmaports" "$pmaports_bytes" 'pmaports source archive'
printf '%s  %s\n' "$pmaports_sha" "$pmaports" | sha256sum --check --status - ||
    vm_die 'pmaports source archive checksum mismatch'

pmaports_root="pmaports-$pmaports_commit"
review_dir="$vm_root/cache/postmarketos-source-review"
if [[ ! -e "$review_dir" ]]; then
    mkdir -- "$review_dir"
    chmod 0700 -- "$review_dir"
fi
vm_assert_private_dir "$review_dir"

review_one() (
    local component=$1 version=$2 package_path=$3 source_url=$4 source_sha512=$5
    local apkbuild destination partial actual_sha actual_bytes source_template
    apkbuild="$pmaports_root/$package_path/APKBUILD"
    tar -tzf "$pmaports" -- "$apkbuild" >/dev/null ||
        vm_die "pmaports lacks reviewed APKBUILD: $package_path"
    tar -xOzf "$pmaports" -- "$apkbuild" |
        grep -Fx "pkgver=$version" >/dev/null ||
        vm_die "pmaports package version drifted: $component"
    source_template=${source_url//"$version"/'$pkgver'}
    tar -xOzf "$pmaports" -- "$apkbuild" |
        grep -F -- "$source_template" >/dev/null ||
        vm_die "pmaports source URL drifted: $component"
    tar -xOzf "$pmaports" -- "$apkbuild" |
        grep -F -- "$source_sha512  $component-$version.tar.gz" >/dev/null ||
        vm_die "pmaports source digest drifted: $component"

    destination="$review_dir/$component-$version.tar.gz"
    if [[ ! -e "$destination" ]]; then
        partial="$(mktemp "$review_dir/.${component}-${version}.partial.XXXXXXXXXX")" ||
            vm_die 'cannot allocate private postmarketOS source review file'
        trap 'rm -f -- "$partial"' EXIT HUP INT TERM
        chmod 0600 -- "$partial"
        bash "$SCRIPT_DIR/run-with-file-limit.sh" 67108864 \
            curl --fail --location --proto '=https' --tlsv1.2 \
            --connect-timeout 15 --max-time 900 --max-filesize 67108864 \
            --retry 0 --output "$partial" -- "$source_url"
        printf '%s  %s\n' "$source_sha512" "$partial" |
            sha512sum --check --status - ||
            vm_die "transitively pinned postmarketOS source mismatch: $component"
        chmod 0400 -- "$partial"
        ln -- "$partial" "$destination" ||
            vm_die 'refusing to replace or race a reviewed source cache entry'
        rm -f -- "$partial"
        trap - EXIT HUP INT TERM
    fi

    [[ -f "$destination" && ! -L "$destination" &&
       "$(vm_stat_mode "$destination")" == 400 ]] ||
        vm_die "unsafe reviewed postmarketOS source: $destination"
    vm_assert_owned "$destination"
    vm_assert_file_size_at_most "$destination" 67108864 'reviewed postmarketOS source'
    printf '%s  %s\n' "$source_sha512" "$destination" |
        sha512sum --check --status - ||
        vm_die "cached reviewed postmarketOS source mismatch: $component"
    actual_sha="$(sha256sum "$destination" | awk '{ print $1 }')"
    actual_bytes="$(stat -c '%s' -- "$destination")"
    printf '%s|%s|%s|%s|%s\n' \
        "$component" "$version" "$source_url" "$actual_sha" "$actual_bytes"
)

review_one postmarketos-mkinitfs 2.11.1 main/postmarketos-mkinitfs \
    'https://gitlab.postmarketos.org/postmarketOS/postmarketos-mkinitfs/-/archive/2.11.1/postmarketos-mkinitfs-2.11.1.tar.gz' \
    a57360095b71e5e215606ade0174ee30f13a7df9190de55f42b96c44b0f08069fa33d2f64767fa859282e8e05c11f1bd571625a231f27ee7b85d3089c85db1fe
review_one boot-deploy 0.23.0 main/boot-deploy \
    'https://gitlab.postmarketos.org/postmarketOS/boot-deploy/-/archive/0.23.0/boot-deploy-0.23.0.tar.gz' \
    f5b2bb096207944b5821065506ed1123be2b70dfabe91572cfa44e986f9f71d712ae0863bd845d9c120623bc0ce5963fb3f278d072b5525b12399592f129277c
review_one buffybox 3.5.1 main/unl0kr \
    'https://gitlab.postmarketos.org/postmarketOS/buffybox/-/archive/3.5.1/buffybox-3.5.1.tar.gz' \
    4558edf2d4f43adcee1d12da359ad5c4b9a3f65eadabc354b945c46a08f51f06e0323e0825953b2f9cf08aeefef3161e32dc359a1733cdc2f033c2d60c2c9b50

# The bootable qemu-aarch64 image is a real postmarketOS software-stack test,
# not a Fairphone hardware model. Audit the exact pinned Fairphone device
# contract separately so a generic QEMU deviceinfo can never again be presented
# as proof that candidate generation is side-effect confined.
fairphone_deviceinfo="$pmaports_root/device/testing/device-fairphone-fp6/deviceinfo"
fairphone_deviceinfo_sha="$(tar -xOzf "$pmaports" -- "$fairphone_deviceinfo" |
    sha256sum | awk '{ print $1 }')"
[[ "$fairphone_deviceinfo_sha" == \
   2e9d77cba8c60cd6a58576cdcc24355d8c9d8a2a750bb3ce0399b79591a7eac9 ]] ||
    vm_die 'pinned Fairphone 6 deviceinfo bytes drifted'
for exact_setting in \
    'deviceinfo_codename="fairphone-fp6"' \
    'deviceinfo_generate_bootimg="true"' \
    'deviceinfo_flash_method="fastboot"' \
    'deviceinfo_flash_kernel_on_update="true"' \
    'deviceinfo_header_version="2"'
do
    tar -xOzf "$pmaports" -- "$fairphone_deviceinfo" |
        grep -Fx -- "$exact_setting" >/dev/null ||
        vm_die "Fairphone 6 deviceinfo lost reviewed setting: $exact_setting"
done

mkinitfs_bootdeploy="$review_dir/postmarketos-mkinitfs-2.11.1.tar.gz"
bootdeploy="$review_dir/boot-deploy-0.23.0.tar.gz"
[[ -f "$mkinitfs_bootdeploy" && -f "$bootdeploy" ]] ||
    vm_die 'reviewed mkinitfs/boot-deploy sources are absent'
tar -xOzf "$mkinitfs_bootdeploy" \
    postmarketos-mkinitfs-2.11.1/internal/bootdeploy/bootdeploy.go |
    grep -F -- '"-o", b.outDir,' >/dev/null ||
    vm_die 'mkinitfs no longer passes the candidate output directory to boot-deploy as reviewed'
tar -xOzf "$mkinitfs_bootdeploy" \
    postmarketos-mkinitfs-2.11.1/cmd/mkinitfs/main.go |
    grep -F -- 'bootDeploy(workDir, *outDir, devinfo)' >/dev/null ||
    vm_die 'mkinitfs no longer passes generated archives through the reviewed boot-deploy boundary'
tar -xOzf "$bootdeploy" boot-deploy-0.23.0/boot-deploy-functions.sh |
    grep -F -- '. /usr/share/misc/source_deviceinfo' >/dev/null ||
    vm_die 'boot-deploy no longer uses the reviewed deviceinfo override helper'
deviceinfo_helper="$pmaports_root/main/devicepkg-utils/source_deviceinfo"
deviceinfo_sources="$(tar -xOzf "$pmaports" -- "$deviceinfo_helper" |
    grep -F 'SOURCE_DEVICEINFO_ROOT' | grep -F 'deviceinfo" ] && .' || true)"
[[ "$deviceinfo_sources" == *'/usr/share/deviceinfo/deviceinfo" ] && .'* &&
   "$deviceinfo_sources" == *'/etc/deviceinfo" ] && .'* ]] ||
    vm_die 'deviceinfo helper lost its reviewed vendor-plus-override contract'
vendor_line="$(printf '%s\n' "$deviceinfo_sources" | grep -n -F '/usr/share/deviceinfo/deviceinfo' | cut -d: -f1)"
override_line="$(printf '%s\n' "$deviceinfo_sources" | grep -n -F '/etc/deviceinfo' | cut -d: -f1)"
[[ "$vendor_line" =~ ^[0-9]+$ && "$override_line" =~ ^[0-9]+$ &&
   "$vendor_line" -lt "$override_line" ]] ||
    vm_die 'deviceinfo helper no longer applies /etc/deviceinfo after the vendor contract'
unset deviceinfo_helper deviceinfo_sources vendor_line override_line
tar -xOzf "$bootdeploy" boot-deploy-0.23.0/boot-deploy-functions.sh |
    grep -F -- '[ "${deviceinfo_flash_kernel_on_update}" = "true" ] || return 0' >/dev/null ||
    vm_die 'boot-deploy raw-flash deviceinfo gate drifted'
raw_writer='d''d'
tar -xOzf "$bootdeploy" boot-deploy-0.23.0/boot-deploy-functions.sh |
    grep -F -- "$raw_writer if=\"\$work_dir/\$bootimg_filename\" of=\"\$boot_partition\" bs=1M" >/dev/null ||
    vm_die 'boot-deploy raw boot-partition write contract drifted'
unset raw_writer
printf 'SART_VM_FAIRPHONE_FP6_RAW_FLASH_CONTRACT_V1|%s|%s\n' \
    "$pmaports_commit" "$fairphone_deviceinfo_sha"
printf 'SART_VM_POSTMARKETOS_PERSISTENT_NO_FLASH_GUARD_CONTRACT_V1|mkinitfs-2.11.1|boot-deploy-0.23.0\n'

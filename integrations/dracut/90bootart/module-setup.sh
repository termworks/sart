#!/bin/bash

check() {
    return 0
}

depends() {
    echo systemd
    return 0
}

install() {
    inst_simple /usr/lib/bootart/bootart \
        /usr/lib/bootart/bootart

    inst_simple "$moddir/bootart-initrd.service" \
        /usr/lib/systemd/system/bootart-initrd.service

    mkdir -p "$initdir/usr/lib/systemd/system/initrd.target.wants"
    ln -s ../bootart-initrd.service \
        "$initdir/usr/lib/systemd/system/initrd.target.wants/bootart-initrd.service"
}

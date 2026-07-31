{
  description = "bootart Rust and isolated QEMU development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs?rev=4c1018dae018162ec878d42fec712642d214fdfa";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };
        lib = pkgs.lib;
        cargoPackage = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package;
        supportedLinux = builtins.elem system [
          "x86_64-linux"
          "aarch64-linux"
        ];
        expectedElfArch =
          if system == "x86_64-linux" then
            "x86_64"
          else if system == "aarch64-linux" then
            "aarch64"
          else
            null;
        bootartSource = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./LICENSE
            ./README.md
            ./src
            # Cargo validates this explicitly declared, feature-gated test
            # target while reading the manifest. It is never selected for the
            # release build and cannot add a release payload.
            ./tests/installer_tests.rs
          ];
        };
        mkBootartStatic =
          {
            rustPlatform,
            buildPackages,
            expectedArchitecture,
            readelfProgram,
          }:
          rustPlatform.buildRustPackage {
            pname = cargoPackage.name;
            version = cargoPackage.version;
            src = bootartSource;

            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [
              "--no-default-features"
              "--bin"
              "bootart"
            ];

            # The repository's Make verification runs tests before packaging.
            # pkgsStatic is a cross package set even when CPU architectures
            # match, so executing test binaries here is toolchain-dependent.
            doCheck = false;
            strictDeps = true;

            # rustc otherwise embeds Nix's per-build temporary directory in
            # panic/source-location strings from vendored crates.
            preBuild = ''
              export RUSTFLAGS="''${RUSTFLAGS:-} --remap-path-prefix=$NIX_BUILD_TOP=/build"
            '';

            nativeBuildInputs = with buildPackages; [
              bash
              binutils
              coreutils
              findutils
              gnugrep
              gnused
            ];

            # Inspect the final, stripped Nix output. Never use ldd here: ldd
            # can execute an untrusted ELF on some systems.
            postFixup = ''
              READELF=${readelfProgram} \
                ${buildPackages.bash}/bin/bash \
                ${./scripts/artifact-inspect.sh} \
                ${lib.escapeShellArg expectedArchitecture} \
                "$out/bin/bootart" "$out/bin"
            '';

            meta = {
              description = "Self-contained Plymouth-style text boot splash";
              license = lib.licenses.mit;
              mainProgram = "bootart";
              platforms = [
                "x86_64-linux"
                "aarch64-linux"
              ];
            };
          };
        bootartStatic = mkBootartStatic {
          rustPlatform = pkgs.pkgsStatic.rustPlatform;
          buildPackages = pkgs.buildPackages;
          expectedArchitecture = expectedElfArch;
          readelfProgram = "${pkgs.buildPackages.binutils}/bin/readelf";
        };
        aarch64StaticPackageSet = pkgs.pkgsCross.aarch64-multiplatform.pkgsStatic;
        bootartStaticAarch64 =
          if system == "aarch64-linux" then
            bootartStatic
          else
            mkBootartStatic {
              rustPlatform = aarch64StaticPackageSet.rustPlatform;
              buildPackages = aarch64StaticPackageSet.buildPackages;
              expectedArchitecture = "aarch64";
              readelfProgram = "${pkgs.buildPackages.binutils}/bin/readelf";
            };
      in
      (lib.optionalAttrs supportedLinux {
        packages.bootart-static = bootartStatic;
        packages.bootart-static-aarch64 = bootartStaticAarch64;
        packages.default = bootartStatic;
        checks.bootart-static = bootartStatic;
      })
      // {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            gnumake
            pkg-config
            clang
            mold
            git-cliff

            # Artifact and initramfs inspection.
            binutils
            diffutils
            file
            cpio
            gzip
            xz
            zstd
            xorriso
            squashfsTools

            # Disposable VM harness.
            # Headless, host-CPU QEMU has no GTK makeCWrapper. The PID that
            # Make records therefore becomes the exact validated QEMU ELF
            # instead of immediately execing an unpinned hidden executable.
            qemu_test
            qemu-utils
            cryptsetup
            curl
            cacert
            jq
            socat
            util-linux
            coreutils
            findutils
            gnugrep
            gnused
            gawk
          ];

          BOOTART_VM_ROOT = "${toString ./.}/target/vm";
        };
      }
    );
}

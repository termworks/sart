{
  description = "bootart static-musl C++23 package and QEMU development shell";

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
        projectLines = lib.filter (line: line != "") (lib.splitString "\n" (builtins.readFile ./PROJECT));
        projectName = builtins.elemAt projectLines 0;
        projectVersion = builtins.elemAt projectLines 1;
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
            ./PROJECT
            ./LICENSE
            ./README.md
            ./Makefile
            ./cpp
            ./scripts/artifact-inspect.sh
          ];
        };
        mkBootartStatic =
          {
            packageSet,
            buildPackages,
            expectedArchitecture,
            readelfProgram,
          }:
          packageSet.stdenv.mkDerivation {
            pname = projectName;
            version = projectVersion;
            src = bootartSource;
            strictDeps = true;
            dontConfigure = true;
            nativeBuildInputs = with buildPackages; [
              bash
              binutils
              coreutils
              gnumake
            ];
            buildInputs = with packageSet; [
              zlib
              zstd.dev
              zstd.out
            ];
            buildPhase = ''
              runHook preBuild
              make cpp-release-build
              runHook postBuild
            '';
            installPhase = ''
              runHook preInstall
              install -Dm755 target/cpp/release/bootart "$out/bin/bootart"
              runHook postInstall
            '';
            postFixup = ''
              READELF=${readelfProgram} \
                ${buildPackages.bash}/bin/bash \
                ${./scripts/artifact-inspect.sh} \
                ${lib.escapeShellArg expectedArchitecture} \
                "$out/bin/bootart" "$out/bin"
            '';
            meta = {
              description = "Self-contained text boot splash";
              license = lib.licenses.mit;
              mainProgram = "bootart";
              platforms = [
                "x86_64-linux"
                "aarch64-linux"
              ];
            };
          };
        bootartStatic = mkBootartStatic {
          packageSet = pkgs.pkgsStatic;
          buildPackages = pkgs.buildPackages;
          expectedArchitecture = expectedElfArch;
          readelfProgram = "${pkgs.pkgsStatic.stdenv.cc.bintools.bintools}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}readelf";
        };
        aarch64StaticPackageSet = pkgs.pkgsCross.aarch64-multiplatform.pkgsStatic;
        bootartStaticAarch64 =
          if system == "aarch64-linux" then
            bootartStatic
          else
            mkBootartStatic {
              packageSet = aarch64StaticPackageSet;
              buildPackages = aarch64StaticPackageSet.buildPackages;
              expectedArchitecture = "aarch64";
              readelfProgram = "${aarch64StaticPackageSet.stdenv.cc.bintools.bintools}/bin/${aarch64StaticPackageSet.stdenv.cc.targetPrefix}readelf";
            };
      in
      (lib.optionalAttrs supportedLinux {
        packages.bootart-static = bootartStatic;
        packages.bootart-static-aarch64 = bootartStaticAarch64;
        packages.bootart-cpp-static = bootartStatic;
        packages.default = bootartStatic;
        checks.bootart-static = bootartStatic;
        checks.bootart-cpp-static = bootartStatic;
      })
      // {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            gnumake
            pkg-config
            clang
            mold
            git-cliff
            pkgs.pkgsStatic.stdenv.cc
            pkgs.pkgsStatic.stdenv.cc.bintools.bintools
            binutils
            diffutils
            file
            cpio
            gzip
            xz
            zstd
            zlib
            pkgs.pkgsStatic.zlib
            pkgs.pkgsStatic.zstd.dev
            pkgs.pkgsStatic.zstd.out
            xorriso
            squashfsTools
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
          BOOTART_MUSL_CXX = "${pkgs.pkgsStatic.stdenv.cc}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}g++";
          BOOTART_MUSL_AR = "${pkgs.pkgsStatic.stdenv.cc.bintools.bintools}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}ar";
          BOOTART_MUSL_READELF = "${pkgs.pkgsStatic.stdenv.cc.bintools.bintools}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}readelf";
          BOOTART_MUSL_ZLIB = "${pkgs.pkgsStatic.zlib}";
          BOOTART_MUSL_ZSTD = "${pkgs.pkgsStatic.zstd.out}";
        };
      }
    );
}

{
  description = "sart static-musl C++23 package and QEMU development shell";

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
        makefileLines = lib.splitString "\n" (builtins.readFile ./Makefile);
        makeValue = key:
          let
            prefix = "override ${key} := ";
            matches = lib.filter (line: lib.hasPrefix prefix line) makefileLines;
          in
          if builtins.length matches == 1 then
            lib.removePrefix prefix (builtins.head matches)
          else
            throw "Makefile must define ${key} exactly once";
        projectName = makeValue "PROJECT_NAME";
        projectVersion = makeValue "PROJECT_VERSION";
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
        sartSource = lib.fileset.toSource {
          root = ./.;
          fileset = lib.fileset.unions [
            ./LICENSE
            ./README.md
            ./Makefile
            ./include
            ./src
            ./tests
            ./scripts/artifact-inspect.sh
          ];
        };
        mkSartStatic =
          {
            packageSet,
            buildPackages,
            expectedArchitecture,
            readelfProgram,
          }:
          packageSet.stdenv.mkDerivation {
            pname = projectName;
            version = projectVersion;
            src = sartSource;
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
              install -Dm755 target/cpp/release/sart "$out/bin/sart"
              runHook postInstall
            '';
            postFixup = ''
              READELF=${readelfProgram} \
                ${buildPackages.bash}/bin/bash \
                ${./scripts/artifact-inspect.sh} \
                ${lib.escapeShellArg expectedArchitecture} \
                "$out/bin/sart" "$out/bin"
            '';
            meta = {
              description = "Self-contained text boot splash";
              license = lib.licenses.mit;
              mainProgram = "sart";
              platforms = [
                "x86_64-linux"
                "aarch64-linux"
              ];
            };
          };
        sartStatic = mkSartStatic {
          packageSet = pkgs.pkgsStatic;
          buildPackages = pkgs.buildPackages;
          expectedArchitecture = expectedElfArch;
          readelfProgram = "${pkgs.pkgsStatic.stdenv.cc.bintools.bintools}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}readelf";
        };
        aarch64StaticPackageSet = pkgs.pkgsCross.aarch64-multiplatform.pkgsStatic;
        sartStaticAarch64 =
          if system == "aarch64-linux" then
            sartStatic
          else
            mkSartStatic {
              packageSet = aarch64StaticPackageSet;
              buildPackages = aarch64StaticPackageSet.buildPackages;
              expectedArchitecture = "aarch64";
              readelfProgram = "${aarch64StaticPackageSet.stdenv.cc.bintools.bintools}/bin/${aarch64StaticPackageSet.stdenv.cc.targetPrefix}readelf";
            };
      in
      (lib.optionalAttrs supportedLinux {
        packages.sart-static = sartStatic;
        packages.sart-static-aarch64 = sartStaticAarch64;
        packages.sart-cpp-static = sartStatic;
        packages.default = sartStatic;
        checks.sart-static = sartStatic;
        checks.sart-cpp-static = sartStatic;
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

          SART_VM_ROOT = "${toString ./.}/target/vm";
          SART_MUSL_CXX = "${pkgs.pkgsStatic.stdenv.cc}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}g++";
          SART_MUSL_AR = "${pkgs.pkgsStatic.stdenv.cc.bintools.bintools}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}ar";
          SART_MUSL_READELF = "${pkgs.pkgsStatic.stdenv.cc.bintools.bintools}/bin/${pkgs.pkgsStatic.stdenv.cc.targetPrefix}readelf";
          SART_MUSL_ZLIB = "${pkgs.pkgsStatic.zlib}";
          SART_MUSL_ZSTD = "${pkgs.pkgsStatic.zstd.out}";
        };
      }
    );
}

{
  description = "Hortus Linux Nix 开发环境与构建包（CLI 与 GTK4 + libadwaita 原生界面）";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      remapRustFlags = ''
        export RUSTFLAGS="''${RUSTFLAGS:-} --remap-path-prefix=$NIX_BUILD_TOP=/hortus-build --remap-path-prefix=$PWD=/hortus"
      '';
      mkTestFontsConf =
        pkgs:
        pkgs.makeFontsConf {
          fontDirectories = [
            pkgs.dejavu_fonts.minimal
            pkgs.noto-fonts
            pkgs.noto-fonts-cjk-sans
            pkgs.noto-fonts-color-emoji
          ];
        };
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              rustc
              cargo
              clippy
              rustfmt
              pkg-config
              python3
              # 自动化 GUI 检查在私有 Xvfb 中运行，避免在真实桌面上弹窗。
              xvfb
              xvfb-run
            ];

            buildInputs = with pkgs; [
              gtk4
              libadwaita
              vte-gtk4
              pango
              cairo
              fontconfig
              glib
              librsvg
              webp-pixbuf-loader
              gsettings-desktop-schemas
              adwaita-icon-theme
              hicolor-icon-theme
            ];

            shellHook = ''
              export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}:${pkgs.adwaita-icon-theme}/share:${pkgs.hicolor-icon-theme}/share:''${XDG_DATA_DIRS:-/usr/share}"
            '';
          };
        }
      );

      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          hortus-cli = pkgs.rustPlatform.buildRustPackage {
            pname = "hortus-cli";
            version = "1.0.0";
            src = self;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            cargoBuildFlags = [ "-p" "hortus-cli" ];
            cargoTestFlags = [ "-p" "hortus-cli" ];

            preBuild = remapRustFlags;
            preCheck = ''
              export HOME="$TMPDIR"
              export FONTCONFIG_FILE="${mkTestFontsConf pkgs}"
            '';

            meta = with pkgs.lib; {
              description = "Hortus read-only CLI and MCP server";
              license = with licenses; [ mit asl20 ];
              platforms = [ "x86_64-linux" ];
              mainProgram = "hortus";
            };
          };

          hortus-linux = pkgs.rustPlatform.buildRustPackage {
            pname = "hortus-linux";
            version = "1.0.0";
            src = self;
            cargoRoot = "apps/linux";
            buildAndTestSubdir = "apps/linux";

            cargoLock = {
              lockFile = ./apps/linux/Cargo.lock;
            };

            nativeBuildInputs = with pkgs; [
              pkg-config
              wrapGAppsHook4
            ];

            buildInputs = with pkgs; [
              gtk4
              libadwaita
              vte-gtk4
              pango
              cairo
              fontconfig
              glib
              librsvg
              webp-pixbuf-loader
              gsettings-desktop-schemas
              adwaita-icon-theme
              hicolor-icon-theme
            ];

            gappsWrapperArgs = [
              "--set-default"
              "HORTUS_KATEX_FONTS_DIR"
              "${placeholder "out"}/share/hortus/fonts/KaTeX"
              "--set-default"
              "HORTUS_CLI_DIR"
              "${self.packages.${system}.hortus-cli}/bin"
            ];

            preBuild = remapRustFlags;
            preCheck = ''
              export HOME="$TMPDIR"
              export FONTCONFIG_FILE="${mkTestFontsConf pkgs}"
              if [ -d "$PWD/apps/macos/Resources/Fonts/KaTeX" ]; then
                export HORTUS_KATEX_FONTS_DIR="$PWD/apps/macos/Resources/Fonts/KaTeX"
              elif [ -d "$PWD/../macos/Resources/Fonts/KaTeX" ]; then
                export HORTUS_KATEX_FONTS_DIR="$PWD/../macos/Resources/Fonts/KaTeX"
              fi
            '';

            postInstall = ''
              install -Dm644 apps/linux/data/io.github.groundgrounder.Hortus.desktop \
                "$out/share/applications/io.github.groundgrounder.Hortus.desktop"
              install -Dm644 apps/linux/data/io.github.groundgrounder.Hortus.metainfo.xml \
                "$out/share/metainfo/io.github.groundgrounder.Hortus.metainfo.xml"
              install -Dm644 apps/linux/data/icons/hicolor/scalable/apps/io.github.groundgrounder.Hortus.svg \
                "$out/share/icons/hicolor/scalable/apps/io.github.groundgrounder.Hortus.svg"
              install -Dm644 apps/LICENSE \
                "$out/share/licenses/hortus-linux/LICENSE"
              install -Dm644 apps/linux/data/THIRD_PARTY_NOTICES.txt \
                "$out/share/licenses/hortus-linux/THIRD_PARTY_NOTICES.txt"
              install -d "$out/share/hortus/fonts/KaTeX"
              install -Dm644 apps/macos/Resources/Fonts/KaTeX/KaTeX_*.ttf \
                apps/macos/Resources/Fonts/KaTeX/OFL.txt \
                apps/macos/Resources/Fonts/KaTeX/FONT_NOTICE.txt \
                "$out/share/hortus/fonts/KaTeX/"
            '';

            meta = with pkgs.lib; {
              description = "Hortus native GTK4 + libadwaita Markdown knowledge viewer";
              license = licenses.gpl3Only;
              platforms = [ "x86_64-linux" ];
              mainProgram = "hortus-gtk";
            };
          };

          default = self.packages.${system}.hortus-linux;
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          hortus-cli = self.packages.${system}.hortus-cli;
          hortus-linux = self.packages.${system}.hortus-linux;

          workspace-checks = pkgs.rustPlatform.buildRustPackage {
            pname = "hortus-workspace-checks";
            version = "1.0.0";
            src = self;
            buildType = "debug";

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            auditable = false;
            nativeBuildInputs = with pkgs; [
              clippy
              rustfmt
            ];

            cargoBuildFlags = [ "--workspace" ];
            cargoTestFlags = [ "--workspace" ];

            preCheck = ''
              export HOME="$TMPDIR"
              export FONTCONFIG_FILE="${mkTestFontsConf pkgs}"
            '';

            postCheck = ''
              cargo fmt --all --check
              cargo clippy --workspace --all-targets --offline -- -D warnings
            '';

            installPhase = ''
              mkdir -p "$out"
            '';
          };

          linux-app-checks = pkgs.rustPlatform.buildRustPackage {
            pname = "hortus-linux-checks";
            version = "1.0.0";
            src = self;
            cargoRoot = "apps/linux";
            buildAndTestSubdir = "apps/linux";
            buildType = "debug";

            cargoLock = {
              lockFile = ./apps/linux/Cargo.lock;
            };

            auditable = false;
            nativeBuildInputs = with pkgs; [
              pkg-config
              clippy
              rustfmt
            ];

            buildInputs = with pkgs; [
              gtk4
              libadwaita
              vte-gtk4
              pango
              cairo
              fontconfig
              glib
              librsvg
              webp-pixbuf-loader
              gsettings-desktop-schemas
              adwaita-icon-theme
              hicolor-icon-theme
            ];

            preCheck = ''
              export HOME="$TMPDIR"
              export FONTCONFIG_FILE="${mkTestFontsConf pkgs}"
              if [ -d "$PWD/apps/macos/Resources/Fonts/KaTeX" ]; then
                export HORTUS_KATEX_FONTS_DIR="$PWD/apps/macos/Resources/Fonts/KaTeX"
              elif [ -d "$PWD/../macos/Resources/Fonts/KaTeX" ]; then
                export HORTUS_KATEX_FONTS_DIR="$PWD/../macos/Resources/Fonts/KaTeX"
              fi
            '';

            postCheck = ''
              pushd apps/linux
              cargo fmt --check
              cargo clippy --no-default-features --all-targets --offline -- -D warnings
              cargo clippy --features automation --all-targets --offline -- -D warnings
              cargo test --features automation --offline
              popd
            '';

            installPhase = ''
              mkdir -p "$out"
            '';
          };
        }
      );
    };
}

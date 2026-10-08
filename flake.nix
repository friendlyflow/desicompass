{
  description = "desicompass, the keyboard-driven tiling Wayland compositor for sicompass";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # Splits the build into a dependency derivation keyed on Cargo.lock alone
    # and the crate on top. Vendors git dependencies (smithay) by the rev
    # recorded in Cargo.lock, so there is no hash to keep up to date.
    crane.url = "github:ipetkov/crane";

    # The session runs `sicompass --session` inside the compositor, and the
    # greeter runs inside it too. Following our nixpkgs keeps one nixpkgs in
    # the system closure rather than several.
    sicompass = {
      url = "github:friendlyflow/sicompass/v0.2.0";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.crane.follows = "crane";
    };
    loginsicompass = {
      url = "github:friendlyflow/loginsicompass/v0.2.0";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.crane.follows = "crane";
    };
  };

  outputs = { self, nixpkgs, crane, sicompass, loginsicompass }:
    let
      # Linux only: a Wayland compositor on DRM/KMS, libinput and libseat has
      # nothing to run on anywhere else.
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];

      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      nixpkgsFor = forAllSystems (system: import nixpkgs { inherit system; });

      # Single source of truth for the version.
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

      # What the compositor needs on LD_LIBRARY_PATH at runtime, set by the
      # package wrapper. Dispatch libraries only (libGL is libglvnd, libgbm dlopens a backend), never
      # nixpkgs' `mesa`: see mesaVendor.
      runtimeLibs = pkgs: with pkgs; [
        libGL
        libgbm
        libxkbcommon
        wayland
        libinput
        seatd
        udev
      ];

      # The GL/EGL/GBM *vendor* behind those dispatch libraries: the running
      # system's driver, never nixpkgs' own mesa. With nixpkgs' libgbm on the
      # path and EGL resolving to the system driver, two incompatible Mesa
      # builds ended up in one process on the GBM path (the TTY backend only,
      # which is why it survived nested) and it segfaulted inside
      # libEGL_mesa. Applied as defaults, so on a non-NixOS host, or for a
      # deliberate driver test, the environment still wins.
      mesaVendor = {
        __EGL_VENDOR_LIBRARY_DIRS = "/run/opengl-driver/share/glvnd/egl_vendor.d";
        LIBGL_DRIVERS_PATH = "/run/opengl-driver/lib/dri";
        GBM_BACKENDS_PATH = "/run/opengl-driver/lib/gbm";
      };

      # A sicompass plugin built from its checkout, laid out the way the Store
      # unpacks one: `$out/<name>/` holding the program as `<entry>`, its
      # `plugin.json`, `LICENSE` and `THIRD-PARTY-LICENSES.html` when present,
      # `assets/` and `locales/*.ftl`. The reference for that layout is
      # `sicompass-plugin pack` (`collect_files` in the sicompass-plugin-sdk
      # repo's `src/package.rs`); change the two together.
      #
      # Generic on purpose: every plugin repo is one Cargo package whose bin is
      # its package name, with `name` and `entry` in plugin.json, so nothing in
      # a plugin repo is specific to this. Built with glibc, not the static musl
      # of a release, which only matters for a binary copied between machines.
      #
      # `src` is a checkout, typically `builtins.fetchGit "file:///…"`, which
      # takes the working tree's tracked files and leaves out `target/`,
      # `build/` and `dist/`.
      buildPlugin = pkgs: src:
        let
          craneLib = crane.mkLib pkgs;
          manifest = builtins.fromJSON (builtins.readFile "${src}/plugin.json");
          package = (builtins.fromTOML (builtins.readFile "${src}/Cargo.toml")).package;
          args = {
            inherit src;
            pname = package.name;
            version = package.version or manifest.version or "0";
            strictDeps = true;
            cargoExtraArgs = "--locked --bin ${package.name}";
            # The plugin's own CI runs its suite.
            doCheck = false;
          };
        in
        craneLib.buildPackage (args // {
          cargoArtifacts = craneLib.buildDepsOnly args;
          installPhaseCommand = ''
            dir=$out/${manifest.name}
            mkdir -p "$dir"
            install -Dm755 "''${CARGO_TARGET_DIR:-target}/release/${package.name}" "$dir/${manifest.entry}"
            install -Dm644 plugin.json "$dir/plugin.json"
            for f in LICENSE THIRD-PARTY-LICENSES.html; do
              if [ -f "$f" ]; then install -Dm644 "$f" "$dir/$f"; fi
            done
            if [ -d assets ]; then cp -r assets "$dir/assets"; fi
            if [ -d locales ]; then
              (cd locales && find . -name '*.ftl' -exec install -Dm644 {} "$dir/locales/{}" \;)
            fi
          '';
        });
    in
    {
      devShells = forAllSystems (system:
        let pkgs = nixpkgsFor.${system}; in
        {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              cargo
              rustc
              rust-analyzer
              clippy
              rustfmt
              pkg-config
              # aws-lc-sys, the TLS stack under reqwest, which the superkey's
              # Store downloads plugins with, drives a CMake build.
              cmake

              wayland
              wayland-scanner
              wayland-protocols
              libxkbcommon
              libGL
              libdrm

              # The TTY backend: libinput for input
              # devices, seatd for libseat (nixpkgs has no `libseat` attribute;
              # the daemon package ships the library, and the logind backend
              # is what actually gets used), udev for device enumeration.
              libinput
              seatd
              udev
              # gbm is its own package (mesa-libgbm), no longer part of mesa's
              # output, so `gbm.pc` is only found with this listed.
              libgbm

              # The superkey (lib/lib_superkey) is a sicompass-ui client, so it
              # needs the renderer's stack as well: SDL3 for the window,
              # freetype and libwebp for text and images, the Vulkan loader,
              # and AT-SPI over D-Bus for the screen reader. The same list as
              # the desicompass-superkey package's buildInputs.
              sdl3
              freetype
              libwebp
              vulkan-loader
              vulkan-headers
              at-spi2-core
              dbus

              # Test clients, cheapest first: wayland-info dumps the registry
              # so you can see which globals are advertised, foot is a shm-only
              # terminal that needs no GPU import, and vkcube is the smallest
              # hardware Vulkan client there is, which answers the dmabuf
              # question without dragging sicompass into the diagnosis.
              wayland-utils
              foot
              vulkan-tools
            ];

            shellHook = with pkgs; ''
              export RUST_SRC_PATH="${rustc}/lib/rustlib/src/rust/library";
              export PKG_CONFIG_PATH="${sdl3}/lib/pkgconfig:${libxkbcommon.dev}/lib/pkgconfig:$PKG_CONFIG_PATH";
              export LIBRARY_PATH="${sdl3}/lib:${libxkbcommon}/lib:${wayland}/lib:${libGL}/lib:${libinput}/lib:${seatd}/lib:${udev}/lib:${libgbm}/lib:$LIBRARY_PATH";
              export VULKAN_SDK="${vulkan-headers}";

              # smithay's backend_egl dlopens libEGL.so.1 and libGLESv2.so.2 by
              # bare name, so the dispatch libraries have to be on the path or
              # the compositor links fine and dies at startup. Store paths only:
              # LD_LIBRARY_PATH outranks every binary's RUNPATH, and a system
              # lib dir here breaks the shell on a distro with an older glibc.
              # The superkey adds the Vulkan loader (dlopened by ash, so it is
              # not in DT_NEEDED), SDL3, freetype and libwebp: the loader and
              # dispatch side only, never a vendor.
              export LD_LIBRARY_PATH="${libxkbcommon}/lib:${wayland}/lib:${libGL}/lib:${libinput}/lib:${seatd}/lib:${udev}/lib:${libgbm}/lib:${vulkan-loader}/lib:${sdl3}/lib:${freetype}/lib:${libwebp}/lib";

              # The GL/EGL/GBM *vendor*, as opposed to the dispatch libraries
              # above. On NixOS it must be /run/opengl-driver, never nixpkgs'
              # own mesa: two Mesa builds in one process segfault on the first
              # call across the boundary (observed on the TTY backend, in
              # eglQueryDmaBufModifiersEXT).
              if [ -d /run/opengl-driver/lib ]; then
                export __EGL_VENDOR_LIBRARY_DIRS="/run/opengl-driver/share/glvnd/egl_vendor.d";
                export LIBGL_DRIVERS_PATH="/run/opengl-driver/lib/dri";
                export GBM_BACKENDS_PATH="/run/opengl-driver/lib/gbm";
              else
                export __EGL_VENDOR_LIBRARY_DIRS="${mesa}/share/glvnd/egl_vendor.d";
              fi

              # Hand interactive sessions to the user's login shell. $SHELL is
              # useless here (nix develop overwrites it with its own bash), so
              # ask the user database. The [ -t 0 ] guard is load-bearing:
              # without it `nix develop -c <cmd>` would exec a shell in place of
              # the command, which then silently never runs.
              if [ -t 0 ]; then
                _sh="$SICOMPASS_DEV_SHELL";
                if [ -z "$_sh" ] && [ -r /etc/passwd ]; then
                  _sh=$(awk -F: -v u="$(id -un)" '$1 == u { print $7 }' /etc/passwd);
                fi
                case "''${_sh##*/}" in
                  bash | "") ;;
                  *) command -v "$_sh" >/dev/null 2>&1 && exec "$_sh" ;;
                esac
                unset _sh;
              fi
            '';
          };
        });

      packages = forAllSystems (system:
        let
          pkgs = nixpkgsFor.${system};
          lib = pkgs.lib;
          craneLib = crane.mkLib pkgs;
          commonArgs = {
            inherit version;
            pname = "desicompass";
            src = craneLib.cleanCargoSource ./.;
            strictDeps = true;
            # Only the compositor. The workspace's default members include the
            # superkey, which links SDL3 and Vulkan; the compositor must not.
            cargoExtraArgs = "--locked -p desicompass";
            # The suite runs in ci.yml and locally.
            doCheck = false;
            nativeBuildInputs = with pkgs; [ pkg-config ];
            buildInputs = with pkgs; [
              wayland
              libxkbcommon
              libinput
              seatd
              udev
              libgbm
              libdrm
              libGL
            ];
          };
        in
        rec {
          desicompass = craneLib.buildPackage (commonArgs // {
            cargoArtifacts = craneLib.buildDepsOnly commonArgs;
            nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.makeWrapper ];

            # Only the dispatch libraries go on LD_LIBRARY_PATH, and the
            # GL/EGL/GBM *vendor* is pointed at /run/opengl-driver. Both halves
            # are required, see runtimeLibs and mesaVendor.
            #
            # `--set-default` rather than `--set`, so on a non-NixOS host, or
            # for a deliberate driver test, the environment still wins.
            #
            # The superkey is part of desicompass: DESICOMPASS_SUPERKEY points
            # the compositor at this flake's own build of it, so every session
            # has one with nothing to configure. A default, so the login
            # screen's empty value (no superkey before signing in) still wins.
            # The bar, DESICOMPASS_BAR, the same way.
            postInstall = ''
              wrapProgram $out/bin/desicompass \
                --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)}" \
                --set-default DESICOMPASS_SUPERKEY ${desicompass-superkey}/bin/desicompass-superkey \
                --set-default DESICOMPASS_BAR ${desicompass-bar}/bin/desicompass-bar \
                ${pkgs.lib.concatStringsSep " \\\n  " (pkgs.lib.mapAttrsToList
                  (name: value: "--set-default ${name} ${value}") mesaVendor)}
            '';

            meta = with pkgs.lib; {
              description = "Keyboard-driven tiling Wayland compositor for sicompass";
              homepage = "https://github.com/friendlyflow/desicompass";
              license = licenses.gpl3Only;
              mainProgram = "desicompass";
              platforms = platforms.linux;
            };
          });
          default = desicompass;

          # The superkey, a sicompass-ui client like loginsicompass, and built
          # the same way: see that flake for the reasoning behind each input
          # and each line of the wrapper.
          desicompass-superkey =
            let
              superkeyArgs = {
                inherit version;
                pname = "desicompass-superkey";
                # The locale bundles are compiled in with include_str!, and the
                # font licenses are installed below and read by the tests.
                src = lib.fileset.toSource {
                  root = ./.;
                  fileset = lib.fileset.unions [
                    (craneLib.fileset.commonCargoSources ./.)
                    ./lib/lib_superkey/locales
                    ./lib/lib_superkey/fonts
                    ./THIRD-PARTY-LICENSES.html
                  ];
                };
                strictDeps = true;
                cargoExtraArgs = "--locked -p desicompass-superkey";
                # tests/superkey_ui.rs drives the real renderer; run it in the
                # dev shell.
                doCheck = false;
                # cmake: aws-lc-sys, the TLS stack under reqwest, which the
                # Store section (sicompass-store) downloads plugins with.
                nativeBuildInputs = with pkgs; [ pkg-config rustPlatform.bindgenHook cmake ];
                buildInputs = with pkgs; [
                  sdl3
                  freetype
                  libwebp
                  libxkbcommon
                  wayland
                  at-spi2-core
                  dbus
                  libGL
                  libgbm
                  libdrm
                ];
              };
            in
            craneLib.buildPackage (superkeyArgs // {
              cargoArtifacts = craneLib.buildDepsOnly superkeyArgs;
              nativeBuildInputs = superkeyArgs.nativeBuildInputs ++ [ pkgs.makeWrapper ];

              # The Vulkan loader and the dispatch libraries on the path, the
              # vendor from /run/opengl-driver: never nixpkgs' own mesa (see
              # mesaVendor).
              postInstall = ''
                wrapProgram $out/bin/desicompass-superkey \
                  --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath (with pkgs; [
                    vulkan-loader
                    sdl3
                    libGL
                    libgbm
                    libxkbcommon
                    wayland
                  ])}" \
                  ${lib.concatStringsSep " \\\n  " (lib.mapAttrsToList
                    (name: value: "--set-default ${name} ${value}") mesaVendor)}

                # The fonts are inside the binary (through sicompass-ui), so
                # their licenses travel with it.
                install -Dm644 lib/lib_superkey/fonts/LICENSE-DejaVu.txt \
                  $out/share/doc/desicompass-superkey/LICENSE-DejaVu.txt
                install -Dm644 lib/lib_superkey/fonts/LICENSE-NotoColorEmoji.txt \
                  $out/share/doc/desicompass-superkey/LICENSE-NotoColorEmoji.txt
                install -Dm644 THIRD-PARTY-LICENSES.html \
                  $out/share/doc/desicompass-superkey/THIRD-PARTY-LICENSES.html
              '';

              meta = with lib; {
                description = "The superkey of the desicompass session";
                homepage = "https://github.com/friendlyflow/desicompass";
                license = licenses.gpl3Only;
                mainProgram = "desicompass-superkey";
                platforms = platforms.linux;
              };
            });

          # The bar, a sicompass-ui client built exactly like the superkey. It
          # runs `spd-say` (Super+D) and `wpctl` (the volume) from the
          # session's PATH rather than bundling them, so they are the ones
          # that match the running speech-dispatcher and PipeWire. The module
          # enables Orca, which brings speech-dispatcher; without `wpctl`
          # there is simply no volume icon.
          desicompass-bar =
            let
              barArgs = {
                inherit version;
                pname = "desicompass-bar";
                # The font licenses are installed below and read by the tests.
                src = lib.fileset.toSource {
                  root = ./.;
                  fileset = lib.fileset.unions [
                    (craneLib.fileset.commonCargoSources ./.)
                    ./lib/lib_bar/fonts
                    ./THIRD-PARTY-LICENSES.html
                  ];
                };
                strictDeps = true;
                cargoExtraArgs = "--locked -p desicompass-bar";
                # tests/dbus.rs starts a dbus-daemon; run the suite in the dev
                # shell.
                doCheck = false;
                nativeBuildInputs = with pkgs; [ pkg-config rustPlatform.bindgenHook ];
                buildInputs = with pkgs; [
                  sdl3
                  freetype
                  libwebp
                  libxkbcommon
                  wayland
                  at-spi2-core
                  dbus
                  libGL
                  libgbm
                  libdrm
                ];
              };
            in
            craneLib.buildPackage (barArgs // {
              cargoArtifacts = craneLib.buildDepsOnly barArgs;
              nativeBuildInputs = barArgs.nativeBuildInputs ++ [ pkgs.makeWrapper ];

              # The same wrapper as the superkey's: the Vulkan loader and the
              # dispatch libraries on the path, the vendor from
              # /run/opengl-driver (see mesaVendor).
              postInstall = ''
                wrapProgram $out/bin/desicompass-bar \
                  --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath (with pkgs; [
                    vulkan-loader
                    sdl3
                    libGL
                    libgbm
                    libxkbcommon
                    wayland
                  ])}" \
                  ${lib.concatStringsSep " \\\n  " (lib.mapAttrsToList
                    (name: value: "--set-default ${name} ${value}") mesaVendor)}

                install -Dm644 lib/lib_bar/fonts/LICENSE-DejaVu.txt \
                  $out/share/doc/desicompass-bar/LICENSE-DejaVu.txt
                install -Dm644 lib/lib_bar/fonts/LICENSE-NotoColorEmoji.txt \
                  $out/share/doc/desicompass-bar/LICENSE-NotoColorEmoji.txt
                install -Dm644 THIRD-PARTY-LICENSES.html \
                  $out/share/doc/desicompass-bar/THIRD-PARTY-LICENSES.html
              '';

              meta = with lib; {
                description = "The bar of the desicompass session";
                homepage = "https://github.com/friendlyflow/desicompass";
                license = licenses.gpl3Only;
                mainProgram = "desicompass-bar";
                platforms = platforms.linux;
              };
            });
        });

      # `buildPlugin pkgs src`, for a configuration that wants a plugin built
      # outside `services.desicompass.dev.plugins`.
      lib = { inherit buildPlugin; };

      # The plugin builder against a minimal plugin tree, so CI exercises it
      # without fetching a plugin repo.
      checks = forAllSystems (system:
        let
          pkgs = nixpkgsFor.${system};
          built = buildPlugin pkgs ./tests/fixture-plugin;
        in
        {
          plugin-layout = pkgs.runCommand "plugin-layout" { } ''
            test -x ${built}/fixture/plugin
            test -f ${built}/fixture/plugin.json
            test -f ${built}/fixture/locales/en-US.ftl
            test -f ${built}/fixture/assets/note.txt
            test -f ${built}/fixture/LICENSE
            test ! -e ${built}/fixture/locales/notes.txt
            test ! -e ${built}/bin
            touch $out
          '';
        });

      # Opt-in NixOS integration. Enabling nothing changes nothing.
      #
      # Two steps on purpose, and the order matters on a machine someone
      # depends on:
      #
      #   services.desicompass.enable = true;
      #     Adds "Desicompass" to the session list the *existing* greeter
      #     offers. If the session fails to start you are returned to that
      #     greeter, so a broken session costs a login attempt and nothing
      #     more.
      #
      #   services.desicompass.greeter.enable = true;
      #     Replaces the greeter itself with loginsicompass. Only worth
      #     turning on once the session above is known to work, because a
      #     greeter that fails to start leaves no graphical way in at all -
      #     recovery is a VT and `nixos-rebuild --rollback`.
      #
      #   services.desicompass.dev.enable = true;
      #     For development: adds "Desicompass (dev)" next to "Desicompass",
      #     running a second pair of packages, typically built from local
      #     working trees while the stable entry runs a release. A broken
      #     build costs a login attempt, and the stable session is one entry
      #     away. `dev.plugins` adds plugin checkouts to it, built the same
      #     way, in place of the Store's copies.
      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.desicompass;
          system = pkgs.stdenv.hostPlatform.system;
          desicompassPkg = cfg.package;
          sicompassPkg = cfg.sicompassPackage;
          loginsicompassPkg = cfg.greeter.package;
        in
        {
          options.services.desicompass = {
            enable = lib.mkEnableOption
              "the desicompass session, offered by whichever greeter is configured";

            greeter.enable = lib.mkEnableOption
              "loginsicompass as the greetd greeter, replacing the current one";

            # The three packages are options so that a configuration can build
            # them from its own checkouts (for example a local working tree
            # read with `builtins.getFlake "git+file://..."`) instead of the
            # revisions this flake's lock file pins.
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${system}.desicompass;
              defaultText = lib.literalExpression "desicompass.packages.\${system}.desicompass";
              description = "The desicompass compositor package.";
            };

            sicompassPackage = lib.mkOption {
              type = lib.types.package;
              default = sicompass.packages.${system}.default;
              defaultText = lib.literalExpression "sicompass.packages.\${system}.default";
              description = "The sicompass package the session runs (`sicompass --session`).";
            };

            greeter.package = lib.mkOption {
              type = lib.types.package;
              default = loginsicompass.packages.${system}.default;
              defaultText = lib.literalExpression "loginsicompass.packages.\${system}.default";
              description = "The loginsicompass greeter package.";
            };

            # The dev session's own pair of packages. Built by Nix like the
            # stable ones, so a change reaches it with `nixos-rebuild switch`.
            # Read through `git+file://`, a working tree's uncommitted edits
            # to tracked files are included and untracked files are not.
            dev = {
              enable = lib.mkEnableOption ''
                a second session, "Desicompass (dev)", that runs `dev.package`
                and `dev.sicompassPackage` beside the stable session'';

              package = lib.mkOption {
                type = lib.types.package;
                default = self.packages.${system}.desicompass;
                defaultText = lib.literalExpression "desicompass.packages.\${system}.desicompass";
                description = ''
                  The compositor the dev session runs. The default is the
                  desicompass this module was imported from, so importing it
                  from a working tree makes that tree the dev compositor.
                '';
              };

              sicompassPackage = lib.mkOption {
                type = lib.types.package;
                example = lib.literalExpression
                  ''(builtins.getFlake "git+file:///home/alice/src/friendlyflow/sicompass").packages.''${system}.default'';
                description = ''
                  The sicompass the dev session runs. No default, because the
                  one this flake's lock file pins is the release the stable
                  session already has.
                '';
              };

              plugins = lib.mkOption {
                type = lib.types.listOf lib.types.path;
                default = [ ];
                example = lib.literalExpression ''
                  map (n: builtins.fetchGit "file:///home/alice/src/friendlyflow/''${n}-plugin-sicompass")
                    [ "terminal" "notes" ]'';
                description = ''
                  Plugin checkouts the dev session runs in place of the Store's
                  copies. Each is built with `buildPlugin` and handed to
                  sicompass through `SICOMPASS_PLUGIN_PATH`, so it wins over
                  the user's copy of the same plugin, runs without asking for
                  approval, and the Store leaves it alone. The stable session
                  keeps the Store's copies. List only the plugins you are
                  working on: the others keep their Store copies, and each one
                  listed is compiled when its checkout changes.
                '';
              };
            };

            # The accessibility defaults for the whole machine, written to
            # /etc/sicompass/accessibility.json. The login screen, sicompass
            # and the superkey all read that file, and all use it only for
            # what nobody has chosen: a choice made in any of them is saved to
            # /var/lib/sicompass/accessibility.json, the one object they share
            # both ways (a standalone sicompass keeps its own in settings.json).
            # null means "no opinion", so each program keeps its own default.
            # The greeter's differs from the app's for screenReader: it starts
            # Orca on first use.
            accessibility = {
              screenReader = lib.mkOption {
                type = lib.types.nullOr lib.types.bool;
                default = null;
                example = true;
                description = ''
                  Whether Orca starts at the login screen and in the
                  desicompass session. Unset, the login screen starts it until
                  someone turns it off there, and the session does not.
                '';
              };

              fontScale = lib.mkOption {
                type = lib.types.nullOr (lib.types.enum [
                  "1.00" "1.25" "1.50" "1.75" "2.00" "2.25" "2.50"
                ]);
                default = null;
                example = "2.00";
                description = "The interface scale.";
              };

              colorScheme = lib.mkOption {
                type = lib.types.nullOr (lib.types.enum [ "dark" "light" ]);
                default = null;
                description = "The color scheme.";
              };

              language = lib.mkOption {
                type = lib.types.nullOr (lib.types.enum [ "en-US" "nl-BE" "fr-BE" "de-BE" ]);
                default = null;
                example = "nl-BE";
                description = "The interface language, which is also the screen reader's voice.";
              };

              shoulderSurfingProtection = lib.mkOption {
                type = lib.types.nullOr lib.types.bool;
                default = null;
                description = ''
                  Keep the screen blank while the screen reader goes on
                  working.
                '';
              };
            };

            xkbLayout = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              example = "be";
              description = ''
                Keyboard layout the compositor compiles and hands to every
                client, overriding the system's. Leave it null: desicompass
                then asks systemd-localed, which on NixOS reports
                services.xserver.xkb.{layout,variant,model,options}, all four
                of them rather than only the layout. Set it only to give
                desicompass a different layout from the rest of the system.
              '';
            };
          };

          config =
            let
              # The session entry a display manager offers in its list.
              #
              # Generated here rather than as a package because it points at
              # store paths and may carry the xkbLayout override. Without the
              # override, desicompass reads the layout from systemd-localed at
              # startup (src/xkb.rs), the same source COSMIC's first-login
              # setup copies from.
              #
              # systemd-cat is what makes a failure visible at all. greetd
              # captures neither stdout nor stderr of the session it starts,
              # so a session dying on startup leaves behind only "session
              # opened" and "session closed" a second apart. `journalctl -t
              # desicompass -b` reads it.
              #
              # dbus-run-session is load-bearing. accesskit_unix speaks
              # AT-SPI2 over the session bus, so without one sicompass stalls
              # 400ms at startup waiting for a registration that never
              # arrives and is then mute to screen readers.
              #
              # What the compositor starts is a script rather than an argument
              # containing a space. The Desktop Entry spec gives no special
              # meaning to single quotes, so `--startup-cmd '... --session'`
              # was split by the greeter's parser and the compositor was handed
              # a stray `--session` it rejected. The Exec line now contains no
              # quoting at all.
              xkbArgs = lib.optionalString (cfg.xkbLayout != null) " --xkb-layout ${cfg.xkbLayout}";

              # Both entries, the stable one and the dev one, are this with a
              # different pair of packages. `env` goes through env(1), which
              # keeps the Exec line free of quoting.
              mkSessionPackage = { id, name, comment, compositor, app, env ? [ ] }:
                let
                  startupScript = pkgs.writeShellScript "${id}-startup" ''
                    exec ${app}/bin/sicompass --session
                  '';
                  envPrefix = lib.optionalString (env != [ ])
                    "${pkgs.coreutils}/bin/env ${lib.concatStringsSep " " env} ";
                in
                pkgs.writeTextDir "share/wayland-sessions/${id}.desktop" ''
                  [Desktop Entry]
                  Name=${name}
                  Comment=${comment}
                  Exec=${pkgs.systemd}/bin/systemd-cat --identifier=${id} ${pkgs.dbus}/bin/dbus-run-session ${envPrefix}${compositor}/bin/desicompass --backend tty${xkbArgs} --startup-cmd ${startupScript}
                  Type=Application
                  DesktopNames=Desicompass
                '' // {
                  # NixOS requires anything in sessionPackages to declare the
                  # sessions it provides, matching the .desktop file name. Set
                  # at the top level, not under `passthru`: the option type
                  # tests `p ? providedSessions` directly, and `passthru` is
                  # only lifted by mkDerivation, not by `//` on a built
                  # derivation.
                  providedSessions = [ id ];
                };

              # What the compositor starts when it is the *greeter*, as a
              # script for the same reason: it carries an environment as well
              # as a command, and greetd hands `default_session.command` to
              # sh(1) while desicompass hands `--startup-cmd` to `sh -c`.
              #
              # No --user and no --command. The greeter reads users from
              # /etc/passwd (bounded by /etc/login.defs) and sessions from the
              # wayland-sessions directories itself.
              #
              # The XDG_* variables exist because the greeter user's home is
              # /var/empty. sicompass_sdk::platform honours them ahead of
              # $HOME, so every write the renderer makes lands in the tmpfiles
              # directory rather than failing.
              greeterScript = pkgs.writeShellScript "loginsicompass-start" ''
                export XDG_CONFIG_HOME=/var/lib/loginsicompass/xdg/config
                export XDG_STATE_HOME=/var/lib/loginsicompass/xdg/state
                export XDG_DATA_HOME=/var/lib/loginsicompass/xdg/data
                export XDG_CACHE_HOME=/var/lib/loginsicompass/xdg/cache
                exec ${loginsicompassPkg}/bin/loginsicompass \
                  --state-dir /var/lib/loginsicompass \
                  --sessions-dir /run/current-system/sw/share/wayland-sessions \
                  --screen-reader-command ${config.services.orca.package}/bin/orca \
                  --suspend-command  '${pkgs.systemd}/bin/systemctl suspend' \
                  --reboot-command   '${pkgs.systemd}/bin/systemctl reboot' \
                  --poweroff-command '${pkgs.systemd}/bin/systemctl poweroff'
              '';

              sessionPackage = mkSessionPackage {
                id = "desicompass";
                name = "Desicompass";
                comment = "Use your whole computer from the keyboard, with no mouse needed";
                compositor = desicompassPkg;
                app = sicompassPkg;
              };

              # `dev.plugins`, built and joined into one folder of plugins.
              # Built with this flake's nixpkgs, like the dev packages, so the
              # Rust they need does not depend on the system's channel.
              devPlugins = pkgs.symlinkJoin {
                name = "desicompass-dev-plugins";
                paths = map (buildPlugin nixpkgsFor.${system}) cfg.dev.plugins;
              };

              # The dev session, with its own journal identifier so
              # `journalctl -t desicompass-dev` holds only dev runs, and a
              # backtrace on panic, the likeliest way a dev build ends. The
              # compositor passes its environment on to sicompass and the
              # superkey, which both read SICOMPASS_PLUGIN_PATH. A store path
              # has no spaces, so the Exec line stays free of quoting.
              devSessionPackage = mkSessionPackage {
                id = "desicompass-dev";
                name = "Desicompass (dev)";
                comment = "Desicompass and Sicompass as the dev packages build them";
                compositor = cfg.dev.package;
                app = cfg.dev.sicompassPackage;
                env = [ "RUST_BACKTRACE=1" ]
                  ++ lib.optional (cfg.dev.plugins != [ ]) "SICOMPASS_PLUGIN_PATH=${devPlugins}";
              };
            in
            lib.mkMerge [

            {
              # `greeter.enable` on its own would do nothing, because
              # everything below is gated on `cfg.enable`. Say so at build time
              # instead. This sits outside the `mkIf` on purpose, so it still
              # fires when `enable` is false.
              assertions = [
                {
                  assertion = cfg.greeter.enable -> cfg.enable;
                  message =
                    "services.desicompass.greeter.enable requires "
                    + "services.desicompass.enable: the greeter needs the session "
                    + "it offers, and the at-spi2-core that makes it audible.";
                }
                {
                  assertion = cfg.dev.enable -> cfg.enable;
                  message =
                    "services.desicompass.dev.enable requires "
                    + "services.desicompass.enable: the dev session relies on the "
                    + "wayland-sessions link and the at-spi2-core it sets up.";
                }
              ];
            }

            (lib.mkIf cfg.enable {
              services.displayManager.sessionPackages = [ sessionPackage ];

              # accesskit_unix reaches screen readers over AT-SPI2, a D-Bus
              # service. Without it the app renders but is silent to Orca.
              services.gnome.at-spi2-core.enable = true;

              # The accessibility object the login screen and every session
              # share, both ways: /var/lib/sicompass/accessibility.json. The
              # greeter and the users are different accounts, so the directory
              # belongs to a group of them all. setgid, so every file written in
              # it stays in that group, and writers replace the file by a
              # rename, which the directory's write permission allows any
              # member. A new member has it from their next login.
              users.groups.sicompass-a11y.members = lib.attrNames
                (lib.filterAttrs (_: u: u.isNormalUser) config.users.users);
              systemd.tmpfiles.rules = [
                "d /var/lib/sicompass 2775 root sicompass-a11y - -"
              ];

              # Orca and the speech-dispatcher it speaks through. Both the login
              # screen and the session start it themselves (the greeter by its
              # store path, sicompass from PATH), so nothing autostarts here.
              services.orca.enable = lib.mkDefault true;

              # The shared accessibility defaults. See the `accessibility`
              # options for who reads this and what wins over it.
              environment.etc."sicompass/accessibility.json".text = builtins.toJSON
                (lib.filterAttrs (_: v: v != null) cfg.accessibility);

              environment.systemPackages = [
                desicompassPkg
                sicompassPkg

                # The session entry has to be here too, not only in
                # sessionPackages. cosmic-greeter ignores sessionData and scans
                # /run/current-system/sw/share/wayland-sessions instead, which
                # is what systemPackages populates. sessionPackages is still
                # what GDM, SDDM and LightDM consume, so both are kept.
                sessionPackage
              ];

              # ...and systemPackages alone is still not enough: NixOS links
              # only the directories named in pathsToLink into
              # /run/current-system/sw, and share/wayland-sessions is not a
              # default. Without this the entry is installed and invisible,
              # with no error anywhere.
              environment.pathsToLink = [ "/share/wayland-sessions" ];
            })

            (lib.mkIf cfg.dev.enable {
              # Both lists, for the same reason as the stable entry.
              # loginsicompass finds it through systemPackages like any other.
              services.displayManager.sessionPackages = [ devSessionPackage ];
              environment.systemPackages = [ devSessionPackage ];
            })

            (lib.mkIf cfg.greeter.enable {
              # The greeter user nixpkgs' greetd module creates has no home
              # (`/var/empty`), so everything the greeter writes needs
              # somewhere to be: the remembered user and session, the
              # accessibility choices made on the login screen, plus
              # sicompass-ui's config, state and cache via the XDG_* variables
              # in greeterScript.
              systemd.tmpfiles.rules = [
                "d /var/lib/loginsicompass     0755 greeter greeter - -"
                "d /var/lib/loginsicompass/xdg 0700 greeter greeter - -"
              ];

              # The greeter writes the shared accessibility object too.
              users.groups.sicompass-a11y.members = [ "greeter" ];

              services.greetd = {
                enable = true;
                settings.default_session.command = lib.concatStringsSep " " [
                  # greetd captures neither stdout nor stderr of what it
                  # starts. `journalctl -t loginsicompass -b` reads this.
                  "${pkgs.systemd}/bin/systemd-cat --identifier=loginsicompass"
                  # Set here, before the bus, and not only in greeterScript:
                  # the services the bus activates (dconf, the at-spi bus
                  # launcher) inherit the daemon's environment, not the
                  # greeter's. Without this they write under the greeter
                  # user's home, /var/empty, and every GSettings write Orca
                  # and at-spi make fails.
                  "${pkgs.coreutils}/bin/env"
                  "XDG_CONFIG_HOME=/var/lib/loginsicompass/xdg/config"
                  "XDG_STATE_HOME=/var/lib/loginsicompass/xdg/state"
                  "XDG_DATA_HOME=/var/lib/loginsicompass/xdg/data"
                  "XDG_CACHE_HOME=/var/lib/loginsicompass/xdg/cache"
                  # No superkey at the login screen: it would let anyone start
                  # programs, or end the session, before signing in. A variable
                  # rather than a flag, so a compositor older than the superkey
                  # just ignores it.
                  "DESICOMPASS_SUPERKEY="
                  # No bar either: the login screen shows the time itself, and
                  # the rest of a bar (notifications, the tray) is nobody's
                  # before signing in.
                  "DESICOMPASS_BAR="
                  # Load-bearing for the same reason as on the session's Exec
                  # line: without a session bus the greeter is mute to Orca.
                  "${pkgs.dbus}/bin/dbus-run-session"
                  "${desicompassPkg}/bin/desicompass"
                  "--backend tty${xkbArgs}"
                  # The greeter is a Wayland client, so it needs a compositor
                  # of its own, the same shape cage + gtkgreet use.
                  "--startup-cmd ${greeterScript}"
                ];
              };
            })

          ];
        };
    };
}

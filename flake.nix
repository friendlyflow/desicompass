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
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;

      # What the compositor needs on LD_LIBRARY_PATH at runtime, shared by the
      # package wrapper and the dev session so the two cannot drift. Dispatch
      # libraries only (libGL is libglvnd, libgbm dlopens a backend), never
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
              export PKG_CONFIG_PATH="${libxkbcommon.dev}/lib/pkgconfig:$PKG_CONFIG_PATH";
              export LIBRARY_PATH="${libxkbcommon}/lib:${wayland}/lib:${libGL}/lib:${libinput}/lib:${seatd}/lib:${udev}/lib:${libgbm}/lib:$LIBRARY_PATH";

              # smithay's backend_egl dlopens libEGL.so.1 and libGLESv2.so.2 by
              # bare name, so the dispatch libraries have to be on the path or
              # the compositor links fine and dies at startup. Store paths only:
              # LD_LIBRARY_PATH outranks every binary's RUNPATH, and a system
              # lib dir here breaks the shell on a distro with an older glibc.
              export LD_LIBRARY_PATH="${libxkbcommon}/lib:${wayland}/lib:${libGL}/lib:${libinput}/lib:${seatd}/lib:${udev}/lib:${libgbm}/lib";

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
          craneLib = crane.mkLib pkgs;
          commonArgs = {
            inherit version;
            pname = "desicompass";
            src = craneLib.cleanCargoSource ./.;
            strictDeps = true;
            cargoExtraArgs = "--locked";
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
            postInstall = ''
              wrapProgram $out/bin/desicompass \
                --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath (runtimeLibs pkgs)}" \
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
      #     running whatever `cargo build` last left in local checkouts. A
      #     broken build costs a login attempt, and the stable session is one
      #     entry away.
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

            dev = {
              enable = lib.mkEnableOption ''
                a second session, "Desicompass (dev)", that runs the compositor
                and sicompass from local checkouts as `cargo build` left them.
                Either one that has not been built falls back to its package'';

              checkout = lib.mkOption {
                # A string, not a path: a path would be copied into the store at
                # evaluation, and the whole point is reading the checkout at
                # login.
                type = lib.types.strMatching "/.*";
                example = "/home/alice/src/friendlyflow";
                description = ''
                  Absolute path of the directory holding the `desicompass` and
                  `sicompass` checkouts side by side.
                '';
              };

              profile = lib.mkOption {
                type = lib.types.str;
                default = "debug";
                example = "release";
                description = ''
                  The cargo profile directory under `target/` to run from.
                  `debug` is what `cargo build`, `cargo test` and `cargo run`
                  already produce, and keeps debug assertions and overflow
                  checks on.
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

              startupScript = pkgs.writeShellScript "desicompass-startup" ''
                exec ${sicompassPkg}/bin/sicompass --session
              '';

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
                  --suspend-command  '${pkgs.systemd}/bin/systemctl suspend' \
                  --reboot-command   '${pkgs.systemd}/bin/systemctl reboot' \
                  --poweroff-command '${pkgs.systemd}/bin/systemctl poweroff'
              '';

              sessionPackage = pkgs.writeTextDir
 "share/wayland-sessions/desicompass.desktop" ''
                [Desktop Entry]
                Name=Desicompass
                Comment=Use your whole computer from the keyboard, with no mouse needed
                Exec=${pkgs.systemd}/bin/systemd-cat --identifier=desicompass ${pkgs.dbus}/bin/dbus-run-session ${desicompassPkg}/bin/desicompass --backend tty${xkbArgs} --startup-cmd ${startupScript}
                Type=Application
                DesktopNames=Desicompass
              '' // {
                # NixOS requires anything in sessionPackages to declare the
                # sessions it provides, matching the .desktop file name. Set at
                # the top level, not under `passthru`: the option type tests
                # `p ? providedSessions` directly, and `passthru` is only lifted
                # by mkDerivation, not by `//` on a built derivation.
                providedSessions = [ "desicompass" ];
              };

              # The dev session: the binaries in the checkouts, for testing the
              # TTY backend and the login path (greetd, the session bus, the
              # journal) without a nixos-rebuild per change. Each binary falls
              # back to its package when the checkout has not built it, so the
              # one session serves work on the app, the compositor, or both.
              #
              # A cargo-built binary has a RUNPATH into the dev shell's store
              # for what it links, but nothing for what it dlopens, and none of
              # the package wrapper's environment. The scripts put that back.
              # The libraries come from this flake's nixpkgs, which is the one
              # the desicompass dev shell builds against, so the compositor gets
              # the very store paths it was built with.
              devPkgs = nixpkgsFor.${system};
              devBinary = repo:
                lib.escapeShellArg "${cfg.dev.checkout}/${repo}/target/${cfg.dev.profile}/${repo}";

              devStartupScript = pkgs.writeShellScript "desicompass-dev-startup" ''
                app=${devBinary "sicompass"}
                if [ -x "$app" ]; then
                  echo "desicompass-dev: sicompass from $app"
                  # What sicompass's package wrapper sets, less sdl3, which a
                  # cargo build reaches through its RUNPATH.
                  export PATH=${lib.makeBinPath [ devPkgs.xvfb-run ]}''${PATH:+:$PATH}
                  export LD_LIBRARY_PATH=${lib.makeLibraryPath (with devPkgs; [
                    vulkan-loader
                    libxkbcommon
                    wayland
                  ])}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
                  exec "$app" --session
                fi
                echo "desicompass-dev: no $app, running the packaged sicompass"
                exec ${sicompassPkg}/bin/sicompass --session
              '';

              devSessionScript = pkgs.writeShellScript "desicompass-dev-session" ''
                # A panic is the likeliest way a dev build ends, and without
                # this the journal gets its message but not where it happened.
                export RUST_BACKTRACE=1

                compositor=${devBinary "desicompass"}
                if [ -x "$compositor" ]; then
                  echo "desicompass-dev: desicompass from $compositor"
                  export LD_LIBRARY_PATH=${lib.makeLibraryPath (runtimeLibs devPkgs)}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
                  ${lib.concatStringsSep "\n  " (lib.mapAttrsToList
                    (name: value: "export ${name}=\"\${${name}-${value}}\"") mesaVendor)}
                else
                  echo "desicompass-dev: no $compositor, running the packaged desicompass"
                  compositor=${desicompassPkg}/bin/desicompass
                fi
                exec "$compositor" --backend tty${xkbArgs} --startup-cmd ${devStartupScript}
              '';

              # Its own journal identifier, so `journalctl -t desicompass-dev`
              # holds only dev runs. The same systemd-cat and dbus-run-session
              # as the stable entry, for the same reasons.
              devSessionPackage = pkgs.writeTextDir
 "share/wayland-sessions/desicompass-dev.desktop" ''
                [Desktop Entry]
                Name=Desicompass (dev)
                Comment=Desicompass and Sicompass as built in ${cfg.dev.checkout}
                Exec=${pkgs.systemd}/bin/systemd-cat --identifier=desicompass-dev ${pkgs.dbus}/bin/dbus-run-session ${devSessionScript}
                Type=Application
                DesktopNames=Desicompass
              '' // {
                providedSessions = [ "desicompass-dev" ];
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
              # somewhere to be: the remembered user and session, plus
              # sicompass-ui's config, state and cache via the XDG_* variables
              # in greeterScript.
              systemd.tmpfiles.rules = [
                "d /var/lib/loginsicompass     0755 greeter greeter - -"
                "d /var/lib/loginsicompass/xdg 0700 greeter greeter - -"
              ];

              # Orca, so the accessibility toggle has a screen reader to start.
              environment.systemPackages = [ pkgs.orca ];

              services.greetd = {
                enable = true;
                settings.default_session.command = lib.concatStringsSep " " [
                  # greetd captures neither stdout nor stderr of what it
                  # starts. `journalctl -t loginsicompass -b` reads this.
                  "${pkgs.systemd}/bin/systemd-cat --identifier=loginsicompass"
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

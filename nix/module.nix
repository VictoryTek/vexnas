{ config, lib, pkgs, ... }:
let
  cfg = config.services.vexnas;
  settingsFormat = pkgs.formats.toml { };

  hasInterfaces = cfg.firewall.interfaces != [ ];

  baseConfig = {
    server = {
      listen = cfg.listenAddresses;
      port = cfg.port;
      assets_path = "${cfg.package}/share/vexnas/assets";
      allowed_cidrs = cfg.allowedCidrs;
    };
    auth = { admin_group = cfg.adminGroup; }
      // lib.optionalAttrs (cfg.viewerGroup != null) { viewer_group = cfg.viewerGroup; };
    paths.data_dir = "/var/lib/vexnas";
    helper.socket = "/run/vexnas/helper.sock";
  } // lib.optionalAttrs (cfg.tls.certFile != null) {
    tls = { cert_file = toString cfg.tls.certFile; key_file = toString cfg.tls.keyFile; };
  };

  configFile = settingsFormat.generate "vexnas.toml" (lib.recursiveUpdate baseConfig cfg.settings);
in
{
  options.services.vexnas = {
    enable = lib.mkEnableOption "vexnas NAS management web UI";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.vexnas;
      defaultText = lib.literalExpression "pkgs.vexnas";
      description = ''
        The vexnas package. Needs the vexnas overlay
        (`nixpkgs.overlays = [ inputs.vexnas.overlays.default ]`), or set it to
        `inputs.vexnas.packages.''${pkgs.stdenv.hostPlatform.system}.vexnas`.
      '';
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 7290;
      description = "HTTPS port.";
    };

    listenAddresses = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ "0.0.0.0" "::" ];
      description = "IP addresses to listen on (one socket each; IPv6 sockets are v6-only).";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Open the port in the firewall (scoped to `firewall.interfaces` when set).
        The application also refuses connections from outside `allowedCidrs`.
      '';
    };

    firewall.interfaces = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "eno1" ];
      description = ''
        Interfaces to scope the firewall rule to. When empty and `openFirewall`
        is set, the rule is global and a warning is emitted.
      '';
    };

    allowedCidrs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [
        "127.0.0.0/8" "::1/128"
        "10.0.0.0/8" "172.16.0.0/12" "192.168.0.0/16" "fc00::/7" "fe80::/10"
        "100.64.0.0/10" "fd7a:115c:a1e0::/48" # Tailscale
      ];
      description = ''
        Source addresses allowed to connect at all (checked before the TLS
        handshake). Defaults: loopback, RFC1918, IPv6 ULA/link-local, Tailscale.
      '';
    };

    adminGroup = lib.mkOption {
      type = lib.types.str;
      default = "wheel";
      description = ''
        Members of this group log in with the admin role. There is no implicit
        first-user admin: users in neither this group nor `viewerGroup` are refused.
      '';
    };

    viewerGroup = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Members of this group get a read-only role. Disabled when null.";
    };

    tls = {
      certFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = ''
          PEM certificate (chain). With `keyFile`, replaces the self-signed
          certificate vexnas otherwise generates on first start.
        '';
      };
      keyFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "PEM private key matching `certFile`. Must be readable by the vexnas user.";
      };
    };

    settings = lib.mkOption {
      type = settingsFormat.type;
      default = { };
      description = ''
        Extra settings merged (recursively) over the generated
        `/etc/vexnas/config.toml`, e.g. `auth.idle_minutes = 15;`.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = (cfg.tls.certFile == null) == (cfg.tls.keyFile == null);
        message = "services.vexnas.tls.certFile and tls.keyFile must be set together.";
      }
      {
        assertion = builtins.hasAttr cfg.adminGroup config.users.groups;
        message = "services.vexnas.adminGroup = \"${cfg.adminGroup}\" is not a declared group.";
      }
      {
        assertion = cfg.viewerGroup == null || builtins.hasAttr cfg.viewerGroup config.users.groups;
        message = "services.vexnas.viewerGroup is not a declared group.";
      }
      {
        assertion = cfg.viewerGroup != cfg.adminGroup;
        message = "services.vexnas.viewerGroup must differ from adminGroup.";
      }
    ];

    warnings = lib.optional (cfg.openFirewall && !hasInterfaces) ''
      services.vexnas.firewall.interfaces is empty, so the vexnas port is opened
      globally. Set explicit interface names to reduce exposure on multi-network
      hosts (vexnas still refuses sources outside services.vexnas.allowedCidrs).
    '';

    users.users.vexnas = {
      isSystemUser = true;
      group = "vexnas";
      description = "vexnas web service";
    };
    users.groups.vexnas = { };

    # PAM service used by the helper to authenticate logins (default stack: pam_unix).
    security.pam.services.vexnas = { };

    environment.etc."vexnas/config.toml".source = configFile;

    # Socket-activated root helper. The socket (0660 root:vexnas) is the only
    # door; the helper also checks SO_PEERCRED against {root, vexnas}.
    systemd.sockets.vexnasd = {
      description = "vexnas root helper socket";
      wantedBy = [ "sockets.target" ];
      socketConfig = {
        ListenStream = "/run/vexnas/helper.sock";
        SocketMode = "0660";
        SocketUser = "root";
        SocketGroup = "vexnas";
        DirectoryMode = "0755";
      };
    };

    # NOTE (restartIfChanged = false on both services): a rebuild must never
    # restart vexnas while an apply it started is still running. The cost, until
    # the apply engine lands (it restarts these units itself as its last step), is
    # that a rebuild which changes vexnas needs a manual:
    #   sudo systemctl restart vexnasd.socket vexnasd.service vexnas.service
    systemd.services.vexnasd = {
      description = "vexnas root helper";
      requires = [ "vexnasd.socket" ];
      after = [ "vexnasd.socket" ];
      restartIfChanged = false;
      serviceConfig = {
        ExecStart = "${cfg.package}/bin/vexnasd --peer-user vexnas --pam-service vexnas";
        # Root, but boxed in. It needs the shadow file and NSS; nothing else.
        CapabilityBoundingSet = [ "CAP_DAC_OVERRIDE" "CAP_DAC_READ_SEARCH" "CAP_SETUID" "CAP_SETGID" "CAP_AUDIT_WRITE" ];
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateNetwork = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@resources" ];
        UMask = "0077";
      };
    };

    systemd.services.vexnas = {
      description = "vexnas web UI";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "vexnasd.socket" ];
      requires = [ "vexnasd.socket" ];
      restartIfChanged = false; # see note above
      serviceConfig = {
        ExecStart = "${cfg.package}/bin/vexnas-web /etc/vexnas/config.toml";
        User = "vexnas";
        Group = "vexnas";
        StateDirectory = "vexnas";
        StateDirectoryMode = "0700";
        Restart = "on-failure";
        RestartSec = 2;
        # Unprivileged and network-facing: no capabilities, no write access
        # outside its state directory.
        CapabilityBoundingSet = "";
        AmbientCapabilities = "";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        ProtectHostname = true;
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged" "~@resources" ];
        UMask = "0077";
      };
    };

    networking.firewall = lib.mkIf cfg.openFirewall (
      if hasInterfaces then {
        interfaces = lib.genAttrs cfg.firewall.interfaces (_: { allowedTCPPorts = [ cfg.port ]; });
      } else {
        allowedTCPPorts = [ cfg.port ];
      }
    );
  };
}

# NixOS VM test: the whole Phase 0 trust chain over real TLS, real PAM, real
# systemd sandboxing, and a second machine on the network.
#
#   nix build path:.#checks.x86_64-linux.vm-login
{ testers, nixosModule, vexnasPackage }:

testers.runNixOSTest {
  name = "vexnas-login";

  nodes = {
    server = { pkgs, ... }: {
      imports = [ nixosModule ];
      services.vexnas = {
        enable = true;
        package = vexnasPackage;
        # Only loopback may connect: the `client` node must be refused.
        allowedCidrs = [ "127.0.0.0/8" "::1/128" ];
      };
      users.users.alice = { isNormalUser = true; extraGroups = [ "wheel" ]; initialPassword = "alicepw"; };
      users.users.bob = { isNormalUser = true; initialPassword = "bobpw"; };
      environment.systemPackages = [ pkgs.curl pkgs.sqlite ];
    };

    client = { pkgs, ... }: {
      environment.systemPackages = [ pkgs.curl ];
    };
  };

  testScript = ''
    import json

    BASE = "https://localhost:7290"

    def api(method, path, body=None, csrf=None, origin=True, jar="/tmp/jar"):
        cmd = "curl -sk -b %s -c %s -X %s -w '\\n%%{http_code}'" % (jar, jar, method)
        if origin:
            cmd += " -H 'Origin: https://localhost:7290'"
        cmd += " -H 'Content-Type: application/json'"
        if csrf:
            cmd += " -H 'X-CSRF-Token: %s'" % csrf
        if body is not None:
            cmd += " -d '%s'" % json.dumps(body)
        out = server.succeed("%s %s%s" % (cmd, BASE, path))
        text, _, code = out.rpartition("\n")
        try:
            parsed = json.loads(text) if text else None
        except ValueError:
            parsed = text
        return int(code), parsed

    def expect(got, want, what):
        if got != want:
            print(server.execute("journalctl -u vexnasd -u vexnas --no-pager -n 60")[1])
            raise AssertionError("%s: got %r, wanted %r" % (what, got, want))

    start_all()
    server.wait_for_unit("vexnasd.socket")
    server.wait_for_unit("vexnas.service")
    server.wait_for_open_port(7290)

    with subtest("service is up over TLS and sets security headers"):
        assert server.succeed("curl -sk %s/health" % BASE).strip() == "ok"
        headers = server.succeed("curl -skI %s/health" % BASE).lower()
        assert "content-security-policy" in headers, headers
        assert "unsafe-inline" not in headers, headers
        assert "strict-transport-security" in headers, headers

    with subtest("the packaged UI boots under the CSP (inline boot script allowed by hash only)"):
        import base64, hashlib, re
        html = server.succeed("curl -sk %s/" % BASE)
        assert "vexnas-frontend" in html, html
        scripts = re.findall(r"<script(?![^>]*\ssrc=)[^>]*>(.*?)</script>", html, re.S)
        assert len(scripts) == 1, scripts
        digest = base64.b64encode(hashlib.sha256(scripts[0].encode()).digest()).decode()
        csp = [l for l in server.succeed("curl -skI %s/" % BASE).splitlines() if l.lower().startswith("content-security-policy")][0]
        assert "'sha256-%s'" % digest in csp, (digest, csp)
        # every asset the page references is actually served
        for path in re.findall(r'(?:href|from|module_or_path:)\s*=?\s*[\'"](/[^\'"]+\.(?:js|wasm|css))', html):
            server.succeed("curl -skf -o /dev/null %s%s" % (BASE, path))

    with subtest("privilege boundaries"):
        assert server.succeed("stat -c %a:%U:%G /run/vexnas/helper.sock").strip() == "660:root:vexnas"
        assert server.succeed("systemctl show vexnas -p User --value").strip() == "vexnas"
        server.fail("runuser -u vexnas -- cat /etc/shadow")
        assert server.succeed("stat -c %a:%U /var/lib/vexnas").strip() == "700:vexnas"

    with subtest("unauthenticated and bad logins"):
        expect(api("GET", "/api/v1/meta")[0], 401, "meta without session")
        expect(api("POST", "/api/v1/auth/login", {"username": "alice", "password": "nope"}), (401, {"error": "Invalid credentials"}), "wrong password")
        expect(api("POST", "/api/v1/auth/login", {"username": "nobody", "password": "x"})[0], 401, "unknown user")
        # bob authenticates via PAM but is in neither the admin nor the viewer group
        expect(api("POST", "/api/v1/auth/login", {"username": "bob", "password": "bobpw"})[0], 403, "unauthorised group")

    with subtest("login requires a matching Origin"):
        assert api("POST", "/api/v1/auth/login", {"username": "alice", "password": "alicepw"}, origin=False)[0] == 403

    with subtest("admin login, session, helper link"):
        code, body = api("POST", "/api/v1/auth/login", {"username": "alice", "password": "alicepw"})
        assert code == 200, body
        assert body["role"] == "admin", body
        csrf = body["csrf_token"]
        cookie = server.succeed("grep __Host-vexnas /tmp/jar")
        assert "#HttpOnly_" in cookie, cookie  # curl marks HttpOnly cookies this way

        code, meta = api("GET", "/api/v1/meta")
        assert code == 200 and meta["helper"]["ok"] is True, meta

    with subtest("sessions survive a service restart"):
        server.succeed("systemctl restart vexnas.service")
        server.wait_for_open_port(7290)
        assert api("GET", "/api/v1/auth/session")[0] == 200

    with subtest("CSRF protection on writes"):
        assert api("POST", "/api/v1/auth/logout", {})[0] == 403
        assert api("POST", "/api/v1/auth/logout", {}, csrf="wrong")[0] == 403
        assert api("POST", "/api/v1/auth/logout", {}, csrf=csrf)[0] == 200
        assert api("GET", "/api/v1/auth/session")[0] == 401

    with subtest("audit trail"):
        actions = server.succeed("sqlite3 /var/lib/vexnas/vexnas.db 'select action from audit order by id'").split()
        for want in ["login_failed", "login_denied", "login_ok", "logout"]:
            assert want in actions, actions

    with subtest("source allowlist refuses other machines"):
        client.wait_for_unit("multi-user.target")
        client.fail("curl -sk --max-time 10 https://server:7290/health")
  '';
}

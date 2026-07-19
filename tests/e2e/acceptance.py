#!/usr/bin/env python3
import base64
import contextlib
import json
import os
import re
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request


CLUSTER = os.environ["KIND_CLUSTER_NAME"]
KUBECTL = ["kubectl", "--request-timeout=30s", "--context", f"kind-{CLUSTER}"]
DIRECTORY = "/catalog/fixture"
MODEL = {"providerID": "fixture", "modelID": "fixture-model"}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


def log(message):
    print(f"[acceptance] {message}", flush=True)


def fail(message):
    raise AssertionError(message)


def check(condition, message):
    if not condition:
        fail(message)


def kubectl(*args, json_output=False, check_result=True):
    result = subprocess.run(
        KUBECTL + list(args), capture_output=True, text=True, check=False
    )
    if check_result and result.returncode:
        fail(f"kubectl {' '.join(args)} failed: {result.stderr.strip()}")
    if json_output:
        return json.loads(result.stdout)
    return result.stdout.strip()


def resource_absent(namespace, kind, name):
    result = subprocess.run(
        KUBECTL + ["-n", namespace, "get", kind, name],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode == 0:
        return False
    if "NotFound" in result.stderr or "not found" in result.stderr.lower():
        return True
    fail(f"kubectl get {kind}/{name} failed unexpectedly: {result.stderr.strip()}")


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class PortForward:
    def __init__(self, namespace, resource, remote_port):
        self.port = free_port()
        self.process = subprocess.Popen(
            KUBECTL
            + [
                "-n",
                namespace,
                "port-forward",
                resource,
                f"{self.port}:{remote_port}",
                "--address",
                "127.0.0.1",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )

    @property
    def url(self):
        return f"http://127.0.0.1:{self.port}"

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            with contextlib.suppress(subprocess.TimeoutExpired):
                self.process.wait(timeout=3)
            if self.process.poll() is None:
                self.process.kill()


def request(base, path, method="GET", payload=None, data=None, headers=None, expected=(200,), timeout=180):
    body = data
    request_headers = dict(headers or {})
    if payload is not None:
        body = json.dumps(payload, separators=(",", ":")).encode()
        request_headers["Content-Type"] = "application/json"
    req = urllib.request.Request(base + path, data=body, method=method, headers=request_headers)
    try:
        response = OPENER.open(req, timeout=timeout)
        status = response.status
        raw = response.read()
        response_headers = response.headers
    except urllib.error.HTTPError as error:
        status = error.code
        raw = error.read()
        response_headers = error.headers
    if status not in expected:
        fail(f"{method} {path} returned {status}, expected {expected}: {raw.decode('utf-8', 'replace')}")
    content_type = response_headers.get("Content-Type", "")
    if raw and "json" in content_type:
        return status, response_headers, json.loads(raw)
    return status, response_headers, raw


def wait_until(description, callback, timeout=180, interval=1):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = callback()
            if value:
                return value
        except Exception as error:
            last = error
        time.sleep(interval)
    suffix = f": {last}" if last else ""
    fail(f"timed out waiting for {description}{suffix}")


def wait_http(base, path):
    return wait_until(
        path,
        lambda: request(base, path, expected=(200,))[0] == 200,
        timeout=120,
    )


def query(values):
    return "?" + urllib.parse.urlencode(values)


def routed_path(path, workspace=None):
    values = {"workspace": workspace} if workspace else {"directory": DIRECTORY}
    return path + query(values)


def all_strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from all_strings(item)
    elif isinstance(value, dict):
        for item in value.values():
            yield from all_strings(item)


def file_text(central, workspace, path):
    _, _, value = request(
        central, routed_path("/file/content", workspace) + "&path=" + urllib.parse.quote(path)
    )
    if isinstance(value, dict) and isinstance(value.get("content"), str):
        return value["content"]
    fail(f"file API returned unexpected content for {path}: {value}")


def prompt(central, workspace, session, marker):
    _, _, value = request(
        central,
        routed_path(f"/session/{session}/message", workspace),
        method="POST",
        payload={
            "model": MODEL,
            "agent": "build",
            "parts": [{"type": "text", "text": marker}],
        },
    )
    text = "\n".join(all_strings(value))
    check("fixture-turn-complete" in text, f"assistant final output missing for {marker}")
    return value


def shell(central, workspace, session, command):
    _, _, value = request(
        central,
        routed_path(f"/session/{session}/shell", workspace),
        method="POST",
        payload={"agent": "build", "model": MODEL, "command": command},
    )
    return "\n".join(all_strings(value))


def state_snapshot(central, workspace, session):
    def capture(command):
        text = shell(
            central,
            workspace,
            session,
            f"printf 'E2E_CAPTURE_BEGIN\\n'; {command}; printf 'E2E_CAPTURE_END\\n'",
        )
        matches = re.findall(r"E2E_CAPTURE_BEGIN\s*(.*?)\s*E2E_CAPTURE_END", text, re.S)
        check(matches, f"shell response omitted captured output: {text}")
        return matches[-1].strip()

    head = capture("git rev-parse HEAD")
    status = capture("git status --porcelain=v2 -z --untracked-files=all | base64 -w0; printf '\\n'")
    hash_files = ("README.md", "staged.txt", "both.txt", "untracked.txt", "staged-new.txt")
    hashes = []
    for path in hash_files:
        digest = capture(f"sha256sum '{path}' | cut -d' ' -f1")
        hashes.append(f"{digest}  {path}")
    base64.b64decode(status, validate=True)
    check(len(hashes) == 5, f"unexpected state hashes: {hashes}")
    return {"head": head, "status": status, "hashes": hashes}


def workspace_record(gateway, workspace):
    return request(gateway, f"/v1/workspaces/{workspace}")[2]


def pod_for(workspace):
    pods = kubectl(
        "-n", "opencode-sandboxes", "get", "pods", "-o", "json", json_output=True
    )["items"]
    matches = [pod for pod in pods if pod["metadata"]["labels"].get("opencode.dev/preview-key") and workspace in json.dumps(pod)]
    if len(matches) == 1:
        return matches[0]
    records = kubectl("-n", "opencode-sandboxes", "get", "pods", "-o", "json", json_output=True)["items"]
    for pod in records:
        env = pod["spec"]["containers"][0].get("env", [])
        if any(item.get("name") == "OPENCODE_WORKSPACE_ID" and item.get("value") == workspace for item in env):
            return pod
    fail(f"expected one Pod for workspace {workspace}")


def launch(gateway, central, title, project_key="fixture"):
    status, headers, _ = request(
        gateway,
        "/v1/launch",
        method="POST",
        payload={"projectKey": project_key, "gitRef": "main", "sessionName": title},
        expected=(303,),
    )
    check(status == 303, "launch did not redirect")
    session = headers["Location"].rstrip("/").rsplit("/", 1)[-1]

    def launched_workspace():
        status, _, info = request(central, f"/session/{session}", expected=(200, 404))
        return info.get("workspaceID") if status == 200 else None

    workspace = wait_until("launched session workspace binding", launched_workspace)
    wait_until(
        "central workspace sync",
        lambda: request(central, routed_path("/path", workspace), expected=(200, 503))[2]
        if request(central, routed_path("/path", workspace), expected=(200, 503))[0] == 200
        else None,
    )
    return session, workspace


def supervisor_status(pod, token):
    forward = PortForward("opencode-sandboxes", f"pod/{pod}", 4097)
    try:
        return wait_until(
            "supervisor status",
            lambda: request(
                forward.url,
                "/healthz",
                headers={"Authorization": f"Bearer {token}"},
                expected=(200, 503),
            )[2],
            timeout=30,
        )
    finally:
        forward.close()


def main():
    gateway_forward = PortForward("opencode-system", "service/gateway", 8080)
    preview_forward = PortForward("opencode-system", "service/gateway", 8081)
    central_forward = PortForward("opencode-system", "service/central", 4096)
    forwards = [gateway_forward, preview_forward, central_forward]
    try:
        gateway = gateway_forward.url
        central = central_forward.url
        wait_http(gateway, "/readyz")
        wait_http(central, "/global/health")

        log("gateway APIs and environment profile")
        projects = request(gateway, "/v1/projects")[2]
        check(
            [project["key"] for project in projects] == ["fixture", "fixture-nix"],
            "fixture projects are not registered",
        )
        profile_one = b"export FIXTURE_PRIVATE=profile-one\n"
        profile_meta = request(
            gateway, "/v1/projects/fixture/env-profile", method="PUT", data=profile_one
        )[2]
        check(profile_meta["projectKey"] == "fixture", "profile metadata project mismatch")
        check(re.fullmatch(r"[0-9a-f]{64}", profile_meta.get("sha256", "")), "profile SHA-256 is invalid")
        check(profile_meta.get("updatedAt"), "profile update timestamp is missing")
        check("profile-one" not in json.dumps(profile_meta), "profile API leaked secret content")

        log("create central session, workspace, warp, and sandbox")
        session, workspace = launch(gateway, central, "kind rev2 primary")
        record = workspace_record(gateway, workspace)
        pod = pod_for(workspace)
        pod_name = pod["metadata"]["name"]
        pod_uid = pod["metadata"]["uid"]
        service_name = record["target"]["url"].split("//", 1)[1].split(".", 1)[0]
        service = kubectl(
            "-n", "opencode-sandboxes", "get", "service", service_name, "-o", "json", json_output=True
        )
        service_ip = service["spec"]["clusterIP"]
        pvcs = kubectl("-n", "opencode-sandboxes", "get", "pvc", "-o", "json", json_output=True)["items"]
        check(not pvcs, "sandbox namespace contains a workspace PVC")
        volumes = {volume["name"]: volume for volume in pod["spec"].get("volumes", [])}
        check(set(volumes) >= {"workspace", "runtime", "runtime-state", "runtime-auth", "env-profile"}, "sandbox volumes are incomplete")
        check(all("emptyDir" in volumes[name] for name in ("workspace", "runtime", "runtime-state")), "sandbox writable volumes are not emptyDir")
        check("secret" in volumes["runtime-auth"] and "secret" in volumes["env-profile"], "sandbox secret volumes are incorrect")
        check(not pod["spec"].get("automountServiceAccountToken", True), "sandbox mounts a service account token")

        child_forward = PortForward("opencode-sandboxes", f"service/{service_name}", 4096)
        try:
            auth = base64.b64encode(
                f"opencode:{record['target']['password']}".encode()
            ).decode()
            child_health = wait_until(
                "child health",
                lambda: request(
                    child_forward.url,
                    "/global/health",
                    headers={"Authorization": f"Basic {auth}"},
                    expected=(200,),
                )[2],
            )
            request(child_forward.url, "/global/health", expected=(401,))
            central_health = request(central, "/global/health")[2]
            check(child_health["version"] == central_health["version"] == "1.18.3", "central/child version mismatch")
        finally:
            child_forward.close()

        log("upstream routed file, list, search, VCS, skills, and tools")
        check("fixture tracked content" in file_text(central, workspace, "tracked.txt"), "routed file read failed")
        listing = request(central, routed_path("/file", workspace) + "&path=")[2]
        check(any(item.get("name") == "server.py" for item in listing), "routed file list missed server.py")
        search = request(central, routed_path("/find", workspace) + "&pattern=fixture%20tracked")[2]
        check(any(item["path"]["text"] == "tracked.txt" for item in search), "routed text search failed")
        vcs = request(central, routed_path("/vcs", workspace))[2]
        check(vcs.get("default_branch") == "main", f"routed VCS API returned wrong repository: {vcs}")
        check(request(central, routed_path("/vcs/status", workspace))[2] == [], "new checkout is dirty")
        skills = request(central, routed_path("/skill", workspace))[2]
        check(any(item.get("name") == "fixture-skill" for item in skills), "project skill was not discovered")
        tools = request(central, routed_path("/experimental/tool/ids", workspace))[2]
        check("bash" in tools, f"stock tools missing: {tools}")

        log("deterministic stock bash tool turn and project plugin")
        prompt(central, workspace, session, "E2E_STOCK_TOOL")
        wait_until(
            "stock bash tool output",
            lambda: "created-by-stock-bash"
            in file_text(central, workspace, "stock-tool.txt"),
            timeout=30,
        )
        prompt(central, workspace, session, "E2E_PLUGIN")
        wait_until(
            "project plugin output",
            lambda: file_text(central, workspace, "plugin-observed.txt").strip()
            == "fixture-plugin-loaded",
            timeout=30,
        )
        prompt(central, workspace, session, "E2E_RUNTIME")
        wait_until(
            "injected runtime output",
            lambda: file_text(central, workspace, "runtime-observed.txt").strip()
            == "runtime-injected",
            timeout=30,
        )

        log("private environment profile and live update")
        prompt(central, workspace, session, "E2E_ENV")
        wait_until(
            "initial environment profile output",
            lambda: file_text(central, workspace, "env-observed.txt").strip()
            == "profile-one",
            timeout=30,
        )
        check(".envrc" not in json.dumps(request(central, routed_path("/vcs/status", workspace))[2]), "managed .envrc appeared in Git status")
        secret_name = next(
            volume["secret"]["secretName"]
            for volume in pod["spec"]["volumes"]
            if volume["name"] == "runtime-auth"
        )
        secret = kubectl(
            "-n", "opencode-sandboxes", "get", "secret", secret_name, "-o", "json", json_output=True
        )
        runtime_token = base64.b64decode(secret["data"]["runtime-token"]).decode()
        initial_supervisor = supervisor_status(pod_name, runtime_token)
        initial_pid = initial_supervisor["childPid"]
        profile_two = b"export FIXTURE_PRIVATE=profile-two\n"
        request(gateway, "/v1/projects/fixture/env-profile", method="PUT", data=profile_two)

        def updated_profile_visible():
            value = shell(
                central,
                workspace,
                session,
                "cat /run/opencode-env/profile.envrc; printf 'VALUE=%s\\n' \"$FIXTURE_PRIVATE\"",
            )
            if "export FIXTURE_PRIVATE=profile-two" in value and "VALUE=profile-two" in value:
                return value
            return None

        wait_until(
            "projected and evaluated environment profile refresh",
            updated_profile_visible,
            timeout=180,
            interval=3,
        )

        def restarted_status():
            value = supervisor_status(pod_name, runtime_token)
            if value.get("childPid") != initial_pid and value.get("ready"):
                return value
            return None

        restarted = wait_until(
            "environment-triggered child restart",
            restarted_status,
            timeout=90,
            interval=2,
        )
        check(pod_for(workspace)["metadata"]["uid"] == pod_uid, "environment refresh replaced Pod")
        check(restarted["restartCount"] >= 1, "supervisor did not record child restart")
        wait_until(
            "workspace resync after environment restart",
            lambda: request(central, routed_path("/path", workspace), expected=(200, 503))[0] == 200,
            timeout=90,
        )
        prompt(central, workspace, session, "E2E_ENV")
        wait_until(
            "updated environment output after restart",
            lambda: file_text(central, workspace, "env-observed.txt").strip()
            == "profile-two",
            timeout=30,
        )

        log("exact checkpoint state and stateless Pod replacement")
        prompt(central, workspace, session, "E2E_CHECKPOINT")
        wait_until(
            "checkpoint fixture mutations",
            lambda: file_text(central, workspace, "staged-new.txt").strip() == "staged-new",
            timeout=30,
        )
        before = state_snapshot(central, workspace, session)
        check(base64.b64decode(before["status"]), "checkpoint fixture unexpectedly has clean status")
        messages_before = request(central, routed_path(f"/session/{session}/message"))[2]
        request(gateway, f"/v1/workspaces/{workspace}/suspend", method="POST")
        wait_until(
            "sandbox Pod deletion",
            lambda: resource_absent("opencode-sandboxes", "pod", pod_name),
            timeout=90,
        )
        latest = request(gateway, f"/v1/workspaces/{workspace}/checkpoints/latest")[2]
        check(latest["head"] == before["head"] and latest["statusSha256"], "checkpoint metadata is incomplete")
        check(
            kubectl("-n", "opencode-sandboxes", "get", "service", service_name, "-o", "jsonpath={.spec.clusterIP}") == service_ip,
            "stable Service changed while suspended",
        )
        check(not kubectl("-n", "opencode-sandboxes", "get", "pvc", "-o", "name"), "workspace PVC appeared after suspend")
        request(gateway, f"/v1/workspaces/{workspace}/ensure", method="POST")
        replacement = pod_for(workspace)
        check(replacement["metadata"]["uid"] != pod_uid, "resume reused old Pod UID")
        check(workspace_record(gateway, workspace)["workspaceId"] == workspace, "workspace ID changed on resume")
        check(kubectl("-n", "opencode-sandboxes", "get", "service", service_name, "-o", "jsonpath={.spec.clusterIP}") == service_ip, "Service identity changed on resume")
        wait_until("workspace resync", lambda: file_text(central, workspace, "untracked.txt"))
        def child_session_ready():
            status, _, _ = request(
                central,
                routed_path(f"/session/{session}/shell", workspace),
                method="POST",
                payload={"agent": "build", "model": MODEL, "command": "git rev-parse --is-inside-work-tree"},
                expected=(200, 404, 503),
                timeout=10,
            )
            return status == 200

        wait_until("session replay after workspace resume", child_session_ready, timeout=90)
        wait_until(
            "restored checkpoint files",
            lambda: file_text(central, workspace, "staged-new.txt").strip() == "staged-new",
            timeout=30,
        )
        after = state_snapshot(central, workspace, session)
        check(after == before, f"restored Git state differs\nbefore={before}\nafter={after}")
        messages_after = request(central, routed_path(f"/session/{session}/message"))[2]
        check(messages_after[: len(messages_before)] == messages_before, "central conversation content or order changed")
        prompt(central, workspace, session, "E2E_CONTINUE")
        wait_until(
            "same central session continuation",
            lambda: file_text(central, workspace, "continued.txt").strip() == "continued-session",
            timeout=30,
        )

        log("preview routing and reserved port rejection")
        prompt(central, workspace, session, "E2E_PREVIEW")
        preview_host = f"{record['target']['url'].split('//', 1)[1].split('.', 1)[0].removeprefix('workspace-')}-18080.test.invalid"
        preview_body = wait_until(
            "preview server",
            lambda: request(
                preview_forward.url,
                "/fixture-health",
                headers={"Host": preview_host},
                expected=(200, 502),
            )[2],
            timeout=30,
        )
        check(preview_body == b"fixture-preview-ok\n", f"preview returned wrong body: {preview_body!r}")
        reserved_host = preview_host.replace("-18080.", "-4096.")
        request(preview_forward.url, "/", headers={"Host": reserved_host}, expected=(400,))

        log("concurrent workspace isolation")
        session_b, workspace_b = launch(gateway, central, "kind rev2 isolated")
        check(workspace_b != workspace, "second launch reused workspace ID")
        prompt(central, workspace, session, "E2E_ISOLATION_A")
        wait_until(
            "workspace A isolation marker",
            lambda: file_text(central, workspace, "isolation-a.txt").strip() == "workspace-a-only",
            timeout=30,
        )
        listing_b = request(
            central, routed_path("/file", workspace_b) + "&path="
        )[2]
        check(
            not any(item.get("name") == "isolation-a.txt" for item in listing_b),
            "workspace A file appeared in workspace B",
        )
        check(pod_for(workspace_b)["metadata"]["uid"] != pod_for(workspace)["metadata"]["uid"], "workspaces share a Pod")

        log("gateway restart recovery and child crash recovery")
        gateway_forward.close()
        kubectl("-n", "opencode-system", "delete", "pod", "-l", "app=gateway", "--wait=true")
        kubectl("-n", "opencode-system", "rollout", "status", "deployment/gateway", "--timeout=180s")
        gateway_forward = PortForward("opencode-system", "service/gateway", 8080)
        forwards[0] = gateway_forward
        gateway = gateway_forward.url
        wait_http(gateway, "/readyz")
        check(workspace_record(gateway, workspace)["state"] == "running", "gateway restart lost workspace state")
        check("fixture tracked content" in file_text(central, workspace, "tracked.txt"), "routing failed after gateway restart")

        current_pod = pod_for(workspace)["metadata"]["name"]
        current_secret = next(
            volume["secret"]["secretName"]
            for volume in pod_for(workspace)["spec"]["volumes"]
            if volume["name"] == "runtime-auth"
        )
        current_token = base64.b64decode(
            kubectl("-n", "opencode-sandboxes", "get", "secret", current_secret, "-o", "json", json_output=True)["data"]["runtime-token"]
        ).decode()
        crash_before = supervisor_status(current_pod, current_token)
        with contextlib.suppress(Exception):
            shell(central, workspace, session, "kill -9 $PPID")

        def child_recovered():
            value = supervisor_status(current_pod, current_token)
            if value.get("ready") and value.get("childPid") != crash_before.get("childPid"):
                return value
            return None

        crash_after = wait_until(
            "supervisor recovery from child crash",
            child_recovered,
            timeout=90,
            interval=2,
        )
        check(crash_after["restartCount"] > crash_before["restartCount"], "child crash did not increment restart count")
        wait_until(
            "session reconnect after child crash",
            lambda: "fixture tracked content" in file_text(central, workspace, "tracked.txt"),
            timeout=90,
        )

        log("abrupt Pod loss and checkpoint recovery")
        abrupt_uid = pod_for(workspace)["metadata"]["uid"]
        kubectl(
            "-n",
            "opencode-sandboxes",
            "delete",
            "pod",
            current_pod,
            "--grace-period=0",
            "--force",
            "--wait=true",
        )
        abrupt_replacement = wait_until(
            "automatic sandbox reconciliation",
            lambda: (
                candidate
                if (candidate := pod_for(workspace))["metadata"]["uid"] != abrupt_uid
                else None
            ),
            timeout=90,
        )
        check(abrupt_replacement["metadata"]["uid"] != abrupt_uid, "abrupt recovery reused the deleted Pod")
        wait_until(
            "session reconnect after abrupt Pod loss",
            lambda: file_text(central, workspace, "untracked.txt").strip() == "untracked",
            timeout=90,
        )
        check(
            request(central, routed_path(f"/session/{session}/message"))[2][: len(messages_before)]
            == messages_before,
            "abrupt Pod loss changed central session history",
        )

        log("generic Nix environment refresh and idle restart")
        nix_session, nix_workspace = launch(
            gateway, central, "kind rev2 nix", project_key="fixture-nix"
        )
        nix_pod = pod_for(nix_workspace)
        nix_pod_uid = nix_pod["metadata"]["uid"]
        nix_pod_name = nix_pod["metadata"]["name"]
        nix_secret = next(
            volume["secret"]["secretName"]
            for volume in nix_pod["spec"]["volumes"]
            if volume["name"] == "runtime-auth"
        )
        nix_token = base64.b64decode(
            kubectl(
                "-n",
                "opencode-sandboxes",
                "get",
                "secret",
                nix_secret,
                "-o",
                "json",
                json_output=True,
            )["data"]["runtime-token"]
        ).decode()
        nix_before = supervisor_status(nix_pod_name, nix_token)

        def nix_environment():
            return shell(
                central,
                nix_workspace,
                nix_session,
                "printf 'NIX_DYNAMIC=%s\\nNIX_CHILD=%s\\n' \"$NIX_FIXTURE_VERSION\" \"$NIX_CHILD_START_VERSION\"",
            )

        initial_nix = wait_until(
            "initial Nix development environment",
            lambda: (value if "NIX_DYNAMIC=one" in (value := nix_environment()) else None),
            timeout=90,
        )
        check("NIX_CHILD=one" in initial_nix, f"child did not start in Nix environment: {initial_nix}")
        prompt(central, nix_workspace, nix_session, "E2E_NIX_CHANGE")
        dynamic_nix = wait_until(
            "live Nix flake refresh",
            lambda: (value if "NIX_DYNAMIC=two" in (value := nix_environment()) else None),
            timeout=180,
            interval=3,
        )
        check("NIX_DYNAMIC=two" in dynamic_nix, "new shell did not evaluate changed flake")
        nix_after = wait_until(
            "Nix child idle restart",
            lambda: (
                value
                if (value := supervisor_status(nix_pod_name, nix_token)).get("ready")
                and value.get("childPid") != nix_before.get("childPid")
                else None
            ),
            timeout=120,
            interval=2,
        )
        check(nix_after["restartCount"] > nix_before["restartCount"], "Nix change did not restart child")
        check(pod_for(nix_workspace)["metadata"]["uid"] == nix_pod_uid, "Nix change replaced Pod")
        wait_until(
            "restarted child inherits changed Nix environment",
            lambda: "NIX_CHILD=two" in nix_environment(),
            timeout=90,
            interval=3,
        )

        log("failure responses and permanent cleanup")
        request(
            gateway,
            "/v1/workspaces",
            method="POST",
            payload={"workspaceId": "bad/id", "projectKey": "fixture", "owner": "e2e@example.test", "upstreamEnvironment": {}},
            expected=(400,),
        )
        request(gateway, f"/v1/workspaces/{workspace}/checkpoints", method="POST", data=b"bad", expected=(401,))
        env_meta = request(gateway, "/v1/projects/fixture/env-profile/meta")[2]
        request(gateway, f"/v1/workspaces/{workspace}", method="DELETE", expected=(204,))
        check(resource_absent("opencode-sandboxes", "pod", current_pod), "deleted workspace Pod remains")
        check(resource_absent("opencode-sandboxes", "service", service_name), "deleted workspace Service remains")
        check(resource_absent("opencode-sandboxes", "secret", current_secret), "deleted runtime Secret remains")
        request(gateway, f"/v1/workspaces/{workspace}/checkpoints/latest", expected=(404,))
        check(request(gateway, "/v1/projects/fixture/env-profile/meta")[2]["sha256"] == env_meta["sha256"], "workspace delete removed environment profile")
        request(gateway, f"/v1/workspaces/{workspace_b}", method="DELETE", expected=(204,))
        request(gateway, f"/v1/workspaces/{nix_workspace}", method="DELETE", expected=(204,))

        log("invalid environment profile fails closed")
        invalid_profile = b"export SHOULD_NOT_EXIST=private-profile-value\nexit 42\n"
        invalid_meta = request(
            gateway,
            "/v1/projects/fixture/env-profile",
            method="PUT",
            data=invalid_profile,
        )[2]
        check("private-profile-value" not in json.dumps(invalid_meta), "invalid profile API leaked secret content")
        invalid_workspace = "wrk_invalid_environment"
        request(
            gateway,
            "/v1/workspaces",
            method="POST",
            payload={
                "workspaceId": invalid_workspace,
                "projectKey": "fixture",
                "gitRef": "main",
                "owner": "e2e@example.test",
                "upstreamEnvironment": {"OPENCODE_AUTH_CONTENT": "{}"},
            },
            expected=(503,),
            timeout=60,
        )
        invalid_record = workspace_record(gateway, invalid_workspace)
        check(invalid_record["state"] == "error", f"invalid environment did not enter error: {invalid_record}")
        error_text = invalid_record.get("error") or ""
        check("environment evaluation failed" in error_text, f"invalid environment error is not actionable: {error_text}")
        check("private-profile-value" not in error_text, "invalid environment error leaked profile content")
        request(gateway, f"/v1/workspaces/{invalid_workspace}", method="DELETE", expected=(204,))
        request(gateway, "/v1/projects/fixture/env-profile", method="DELETE", expected=(204,))
        request(gateway, "/v1/projects/fixture/env-profile/meta", expected=(404,))
        check(session_b != session, "concurrent sessions unexpectedly share identity")
        log("all deterministic kind acceptance checks passed")
    finally:
        for forward in forwards:
            forward.close()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"[acceptance] FAILED: {error}", file=sys.stderr, flush=True)
        raise

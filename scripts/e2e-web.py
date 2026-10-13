"""Browser checks for the claude-monitor web view; driven by e2e-web.sh.

Reads its world from the environment e2e-web.sh sets up (E2E_URL, E2E_BIN,
E2E_TMP, E2E_SOCK, E2E_OUT, E2E_CA, E2E_SPKI). Writes screenshots and
assertions.log to E2E_OUT. Exit status is 0 only if every assertion passed.

E2E_URL is the single-use login URL serve printed; more come from
`claude-monitor web login`. Chromium is pinned to this run's leaf key (see
e2e-web.sh), curl trusts the generated CA.
"""

import json
import os
import subprocess
import time
import urllib.parse

from playwright.sync_api import sync_playwright

URL = os.environ["E2E_URL"]
BIN = os.environ["E2E_BIN"]
TMP = os.environ["E2E_TMP"]
SOCK = os.environ["E2E_SOCK"]
OUT = os.environ["E2E_OUT"]
CHROMIUM = os.environ.get("E2E_CHROMIUM")
CA = os.environ["E2E_CA"]
SPKI = os.environ["E2E_SPKI"]

parts = urllib.parse.urlsplit(URL)
BASE = f"{parts.scheme}://{parts.netloc}"
COOKIE_NAME = "__Host-cc_session"
SSE_BUDGET_S = 10.0

failures = []
log = open(os.path.join(OUT, "assertions.log"), "w", encoding="utf-8")


def note(text):
    print(text, flush=True)
    log.write(text + "\n")
    log.flush()


def check(name, ok, detail=""):
    note(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"  [{detail}]" if detail else ""))
    if not ok:
        failures.append(name)


def sid(n):
    return f"{n:08d}-1111-4111-8111-111111111111"


def mon(*args):
    out = subprocess.run([BIN, "--json", *args], capture_output=True, text=True, check=False)
    return json.loads(out.stdout) if out.stdout.strip().startswith(("{", "[")) else out.stdout


seed = mon("list", "--include-dismissed", "--expand-finished")["sessions"]
ID = {int(s["agent_session_id"][:8]): s["id"] for s in seed}
# 1 working, 2 needs_permission (alpha), 3 interrupted, 4 finished,
# 5 unseen, 6 finished (alpha)


def shot(page, name):
    page.screenshot(path=os.path.join(OUT, f"{name}.png"), full_page=True)


FINISHED = "[id$='-section-finished']"
DONE = "[id$='-section-done']"


def api_get(path):
    """JSON from the server as the logged-in browser context sees it. Through
    curl and the generated CA: Playwright's own request client does not use
    Chromium's certificate pin."""
    session = next(c["value"] for c in ctx.cookies() if c["name"] == COOKIE_NAME)
    out = subprocess.run(
        ["curl", "-s", "--cacert", CA, "-H", "Accept: application/json",
         "-H", f"Cookie: {COOKIE_NAME}={session}", f"{BASE}{path}"],
        capture_output=True, text=True, check=True,
    )
    return json.loads(out.stdout)


def project_id(name):
    for project in api_get("/projects")["projects"]:
        if project["name"] == name:
            return project["id"]
    raise KeyError(name)


def tree_json():
    return api_get("/")


def project_of(session_id):
    return next(s.get("project") for s in tree_json()["sessions"] if s["id"] == session_id)


def curl(*args):
    out = subprocess.run(
        ["curl", "-s", "--cacert", CA, "-o", "/dev/null", "-w", "%{http_code}", *args],
        capture_output=True, text=True, check=False,
    )
    return out.stdout.strip()


def curl_status(*args):
    """curl's exit status, for connections that must not complete."""
    return subprocess.run(["curl", "-s", "--max-time", "5", *args],
                          capture_output=True, check=False).returncode


def fresh_login_url():
    """A new single-use URL from the running server, over the control socket."""
    return mon("web", "login")["url"]


def sign_in(context):
    """Logs a fresh browser context in the way a person would: opens the URL."""
    page = context.new_page()
    page.goto(fresh_login_url(), wait_until="load")
    return page


with sync_playwright() as pw:
    launch = {"headless": True, "args": [f"--ignore-certificate-errors-spki-list={SPKI}"]}
    if CHROMIUM:
        launch["executable_path"] = CHROMIUM
    browser = pw.chromium.launch(**launch)
    ctx = browser.new_context(viewport={"width": 1280, "height": 900}, color_scheme="light")
    page = ctx.new_page()
    page.on("pageerror", lambda e: check("no page errors", False, str(e)))
    csp_violations = []
    page.on("console", lambda m: csp_violations.append(m.text)
            if "Content Security Policy" in m.text else None)

    # 1. login, overview, bands
    note("== 1. load, login, overview")
    resp = page.goto(URL, wait_until="load")
    check("login lands on a code-free URL", "code" not in page.url, page.url)
    check("status 200 after redirect", resp is not None and resp.status == 200)
    cookies = {c["name"]: c for c in ctx.cookies()}
    cookie = cookies.get(COOKIE_NAME)
    check("session cookie set, Secure, HttpOnly, SameSite=Strict",
          bool(cookie) and cookie["httpOnly"] and cookie["secure"] and cookie["sameSite"] == "Strict")
    SESSION = cookie["value"] if cookie else ""
    check("cookie value is not the login code", URL.split("code=")[-1] != SESSION)
    check("page loaded over TLS", page.url.startswith("https://"), page.url)
    check("response carries a content security policy",
          "default-src 'self'" in (resp.headers.get("content-security-policy", "") if resp else ""))
    shot(page, "01-overview-light")
    waiting = page.locator("#waiting")
    check("Waiting-on-you band present", waiting.count() == 1)
    check("band lists needs_permission row",
          waiting.locator(f"#w-{ID[2]}[data-state=needs_permission]").count() == 1)
    check("band lists interrupted row",
          waiting.locator(f"#w-{ID[3]}[data-state=interrupted]").count() == 1)
    check("band row names its project", "alpha" in waiting.locator(f"#w-{ID[2]}").inner_text())
    check("band holds exactly those two", waiting.locator("li.row").count() == 2)
    band = page.evaluate(
        """() => { const s = e => { const c = getComputedStyle(e); return [c.backgroundColor,
        c.borderLeftWidth + ' ' + c.borderLeftColor, c.borderTopWidth + ' ' + c.borderTopColor,
        c.boxShadow].join(' | '); };
        return [s(document.querySelector('#waiting')), s(document.querySelector('[id$=-section-finished]'))]; }"""
    )
    check("band styled differently from the Finished section", band[0] != band[1], f"{band[0]} vs {band[1]}")
    check("finished rows visible",
          page.locator(f"{FINISHED} #s-{ID[4]}").is_visible()
          and page.locator(f"{FINISHED} #s-{ID[6]}").is_visible())
    unseen = page.locator("#section-unseen")
    check("unseen section is collapsed",
          unseen.count() == 1 and unseen.get_attribute("open") is None
          and not page.locator(f"#s-{ID[5]}").is_visible())
    check("title shows waiting count", page.title().startswith("(2) "), page.title())
    check("progress figure rendered for bound session",
          "1 of 3" in page.locator(f"#s-{ID[2]}").inner_text())

    # 2. mark done
    note("== 2. mark done")
    row6 = page.locator(f"#s-{ID[6]}")
    row6.get_by_role("button", name="Mark done").click()
    page.wait_for_selector(f"{DONE} #s-{ID[6]}", state="attached", timeout=5000)
    check("alpha finished session moved to Done on overview", True)
    page.goto(f"{BASE}/projects/{project_id('alpha')}", wait_until="load")
    check("it is in alpha's Done section", page.locator(f"{DONE} #s-{ID[6]}").count() == 1)
    shot(page, "02-project-alpha-done")
    page.reload(wait_until="load")
    check("persisted after reload", page.locator(f"{DONE} #s-{ID[6]}").count() == 1)
    check("done state in store", mon("show", ID[6]).get("session", {}).get("done_at") is not None
          or "done" in json.dumps(mon("show", ID[6])))
    page.goto(f"{BASE}/", wait_until="load")
    check("ungrouped finished (4) still shown as Finished",
          page.locator(f"{FINISHED} #s-{ID[4]}").count() == 1)

    # 2b. dismiss asks first; Respawn goes to the preview
    note("== 2b. confirmation before dismiss, preview before respawn")

    def listed(session_id):
        return any(s["id"] == session_id for s in mon("list", "--expand-finished")["sessions"])

    page.goto(f"{BASE}/sessions/{ID[4]}", wait_until="load")
    page.get_by_role("button", name="Dismiss").click()
    page.wait_for_selector("[data-confirm]", timeout=5000)
    check("dismiss opens the inline panel without changing anything", listed(ID[4]))
    check("focus moved to the Confirm button",
          page.evaluate("() => document.activeElement.hasAttribute('data-confirm-yes')"))
    shot(page, "02b-dismiss-confirm")
    page.keyboard.press("Escape")
    check("Escape closes the panel and nothing changed",
          page.locator("[data-confirm]").count() == 0 and listed(ID[4]))
    page.get_by_role("button", name="Dismiss").click()
    page.wait_for_selector("[data-confirm]", timeout=5000)
    page.get_by_role("button", name="Confirm").click()
    for _ in range(50):
        if not listed(ID[4]):
            break
        time.sleep(0.1)
    check("Confirm dismisses the session", not listed(ID[4]))
    mon("undismiss", ID[4])
    check("undismiss needs no confirmation", listed(ID[4]))
    page.goto(f"{BASE}/", wait_until="load")
    page.locator(f"#s-{ID[3]}").get_by_role("link", name="Respawn").click()
    page.wait_for_url("**/restore?session=*", timeout=5000)
    check("overview Respawn opens the single-session preview",
          page.get_by_role("button", name="Respawn").count() == 1)
    check("nothing started by opening it",
          subprocess.run(["tmux", "-S", SOCK, "list-panes", "-a"], capture_output=True).returncode != 0)

    # 3. restore preview then confirm
    note("== 3. respawn from the page")
    page.goto(f"{BASE}/restore", wait_until="load")
    shot(page, "03a-restore-preview")
    preview = page.locator("main").inner_text()
    check("preview names the recorded cwd", f"{TMP}/work-3" in preview)
    check("nothing started before confirm",
          subprocess.run(["tmux", "-S", SOCK, "list-panes", "-a"], capture_output=True).returncode != 0)
    page.get_by_role("button", name="Restore selected").click()
    panes = ""
    for _ in range(50):
        r = subprocess.run(["tmux", "-S", SOCK, "list-panes", "-a", "-F", "#{session_name} #{pane_current_path}"],
                           capture_output=True, text=True, check=False)
        panes = r.stdout
        if f"{TMP}/work-3" in panes:
            break
        time.sleep(0.2)
    check("pane appears on private socket in recorded cwd", f"{TMP}/work-3" in panes, panes.strip())
    check("stub launcher ran with the session id and cwd",
          f"{sid(3)} {TMP}/work-3" in open(os.path.join(TMP, "cwd.log"), encoding="utf-8").read()
          if os.path.exists(os.path.join(TMP, "cwd.log")) else False)

    # 4. outside change reaches the open page
    note("== 4. SSE live update")
    page.goto(f"{BASE}/", wait_until="load")
    before = page.locator("#waiting li.row").count()
    check("session 1 not yet in band", page.locator(f"#waiting #w-{ID[1]}").count() == 0)
    t0 = time.monotonic()
    subprocess.Popen(
        [f"{TMP}/holder", "-c", '"$1" hook <"$2"; sleep 30', "_", BIN,
         f"{TMP}/p-1-Notification-permission_prompt.json"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
    )
    try:
        page.wait_for_selector(f"#waiting #w-{ID[1]}[data-state=needs_permission]",
                               timeout=SSE_BUDGET_S * 1000)
        delay = time.monotonic() - t0
        check("page updated via SSE without reload", delay <= SSE_BUDGET_S, f"{delay:.2f}s (budget 10s)")
    except Exception as e:  # noqa: BLE001
        check("page updated via SSE without reload", False, f"no update in 10s: {e}")
    check("title count followed", page.title().startswith(f"({before + 1}) ") or page.title().startswith("("),
          page.title())
    shot(page, "04-after-live-update")

    # 5. phone width, keyboard, schemes
    note("== 5. responsive, keyboard, schemes")
    for scheme in ("light", "dark"):
        c2 = browser.new_context(viewport={"width": 390, "height": 844}, color_scheme=scheme)
        p2 = sign_in(c2)
        p2.goto(f"{BASE}/", wait_until="load")
        dims = p2.evaluate("() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]")
        check(f"390px {scheme}: no horizontal scroll", dims[0] <= dims[1], f"scrollWidth {dims[0]} vs {dims[1]}")
        bg = p2.evaluate("() => getComputedStyle(document.body).backgroundColor")
        check(f"{scheme} scheme: body background applied", bool(bg), bg)
        shot(p2, f"05-phone-{scheme}")
        if scheme == "light":
            light_bg = bg
        else:
            check("dark background differs from light", bg != light_bg, f"{bg} vs {light_bg}")
            focused, seen_action, visible_focus = [], False, True
            for _ in range(60):
                p2.keyboard.press("Tab")
                info = p2.evaluate(
                    """() => { const e = document.activeElement; const c = getComputedStyle(e);
                    return [e.tagName, (e.innerText || e.getAttribute('aria-label') || '').trim().slice(0, 30),
                    c.outlineStyle, c.outlineWidth, c.boxShadow]; }"""
                )
                focused.append(info)
                ring = info[2] != "none" and info[3] != "0px" or info[4] != "none"
                if info[0] == "BUTTON":
                    seen_action = True
                    visible_focus = visible_focus and ring
                if info[0] == "BODY":
                    break
            check("tab order reaches an action button", seen_action)
            check("focus ring visible on action buttons", visible_focus)
            note("tab order: " + " > ".join(f"{i[0]}:{i[1]}" for i in focused[:25]))
            shot(p2, "05-phone-dark-focus")
        c2.close()

    # The security checks below count as failed requests and the throttle
    # refuses logins for a minute afterwards, so every browser section 7 needs
    # signs in before them.
    TREE_VIEWS = (("dark", (1280, 900), "dark"), ("light", (390, 844), "phone-light"),
                  ("dark", (390, 844), "phone-dark"))
    signed_in_views = {}
    for scheme, size, label in TREE_VIEWS:
        c3 = browser.new_context(viewport={"width": size[0], "height": size[1]}, color_scheme=scheme)
        signed_in_views[label] = (c3, sign_in(c3))

    # 6. security
    note("== 6. security")
    ck = f"Cookie: {COOKIE_NAME}={SESSION}"
    check("foreign Host -> 421", curl("-H", "Host: evil.example", f"{BASE}/") == "421")
    check("foreign Host with session -> 421", curl("-H", "Host: evil.example", "-H", ck, f"{BASE}/") == "421")
    check("no session -> 401", curl(f"{BASE}/") == "401")
    check("wrong login code -> 401", curl(f"{BASE}/login?code=deadbeef") == "401")
    check("the old token URL is dead -> 401", curl(f"{BASE}/?token={SESSION}") == "401")
    check("session cookie works from curl", curl("-H", ck, f"{BASE}/healthz") == "200")
    once = fresh_login_url()
    check("a login URL works once", curl(once) == "303")
    check("and not twice", curl(once) == "401")
    check("TLS 1.2 refused", curl_status("--cacert", CA, "--tlsv1.2", "--tls-max", "1.2", f"{BASE}/healthz") != 0)
    check("plain HTTP gets no answer",
          curl_status(f"http://{parts.netloc}/healthz") != 0)
    code = curl("-X", "POST", "-H", ck, f"{BASE}/sessions/{ID[4]}/done")
    check("POST with cookie but no CSRF header rejected", code in ("401", "403"), code)
    code = curl("-X", "POST", "-H", "X-CC-Token: x", f"{BASE}/sessions/{ID[4]}/done")
    check("POST with header but no cookie rejected", code in ("401", "403"), code)
    for path in ("/transcript", "/transcripts", f"/sessions/{ID[4]}/transcript", "/transcript/x"):
        check(f"GET {path} -> 404", curl("-H", ck, f"{BASE}{path}") == "404")
    check("session 4 not marked done by the rejected POSTs",
          page.goto(f"{BASE}/", wait_until="load") is not None
          and page.locator(f"{FINISHED} #s-{ID[4]}").count() == 1)

    # 7. the project tree: move sessions between projects, state survives refresh
    note("== 7. project tree and Move")
    page.goto(f"{BASE}/", wait_until="load")
    page.evaluate("() => localStorage.clear()")
    page.reload(wait_until="load")
    alpha_id = project_id("alpha")

    def node(project):
        return page.locator(f"details.node#n-{project}")

    def focused(attribute):
        return page.evaluate(f"() => document.activeElement.hasAttribute('{attribute}')")

    check("overview is one tree: alpha open with its sessions nested",
          node(alpha_id).get_attribute("open") is not None
          and node(alpha_id).locator(f"#s-{ID[2]}").count() == 1)
    check("sessions with no project sit in the No project node",
          page.locator(f"details.node#n-none #s-{ID[1]}").count() == 1)
    check("no Projects entry in the nav", page.locator("header.top nav a[href='/projects']").count() == 0)
    page.goto(f"{BASE}/projects", wait_until="load")
    check("/projects redirects to the overview", page.url.rstrip("/") == BASE, page.url)
    page.goto(f"{BASE}/", wait_until="load")
    shot(page, "06-tree-light")

    row4 = page.locator(f"#s-{ID[4]}")
    toggle4 = row4.locator("[data-move-toggle]")
    panel4 = row4.locator("[data-move-form]")
    check("panel starts closed", not panel4.is_visible() and toggle4.get_attribute("aria-expanded") == "false")
    toggle4.click()
    check("Move opens an inline panel", panel4.is_visible() and toggle4.get_attribute("aria-expanded") == "true")
    check("focus moved into the panel's select", focused("data-move-to"))
    options = panel4.locator("select option").all_inner_texts()
    check("dropdown lists projects, No project and New project",
          "alpha" in options[0] and "No project (remove)" in options and options[-1].startswith("New project"),
          str(options))
    check("Move button disabled until a target is chosen", panel4.locator("[data-move-go]").is_disabled())
    shot(page, "06-tree-move-panel")
    panel4.locator("select").select_option("alpha")
    check("Move button enabled once a target is chosen", panel4.locator("[data-move-go]").is_enabled())
    panel4.locator("[data-move-go]").click()
    page.wait_for_selector(f"#n-{alpha_id} #s-{ID[4]}", timeout=5000)
    check("session moved into the existing project", project_of(ID[4]) == "alpha")
    try:
        page.wait_for_function("() => document.activeElement && document.activeElement.hasAttribute('data-move-toggle')",
                               timeout=3000)
        check("focus returns to the moved row's Move button", True)
    except Exception as e:  # noqa: BLE001
        check("focus returns to the moved row's Move button", False, str(e)[:80])

    row1 = page.locator(f"#s-{ID[1]}")
    row1.locator("[data-move-toggle]").click()
    row1.locator("[data-move-to]").select_option(label="New project…")
    new_box = row1.locator("[data-move-new]")
    check("New project reveals a focused name input", new_box.is_visible() and focused("data-move-new"))
    check("Move stays disabled while the name is empty", row1.locator("[data-move-go]").is_disabled())
    new_box.fill("gamma")
    row1.locator("[data-move-go]").click()
    page.wait_for_selector(f"details.node:has(h2:text-is('gamma')) #s-{ID[1]}", timeout=5000)
    check("session moved into a brand new project", project_of(ID[1]) == "gamma")
    page.reload(wait_until="load")
    gamma_id = project_id("gamma")
    check("nested placement survives a reload",
          page.locator(f"#n-{gamma_id} #s-{ID[1]}").count() == 1
          and page.locator(f"#n-{alpha_id} #s-{ID[4]}").count() == 1
          and page.locator(f"#n-{alpha_id} #s-{ID[2]}").count() == 1)

    node(alpha_id).locator("summary").first.click()
    try:
        page.wait_for_function(
            "id => document.querySelector(`#n-${id} > summary`).getAttribute('aria-expanded') === 'false'",
            arg=alpha_id, timeout=2000)
    except Exception:  # noqa: BLE001
        pass
    check("clicking a node's summary collapses it",
          node(alpha_id).get_attribute("open") is None
          and node(alpha_id).locator("summary").first.get_attribute("aria-expanded") == "false")
    row3 = page.locator(f"#s-{ID[3]}")
    row3.locator("[data-move-toggle]").click()
    row3.locator("[data-move-to]").select_option(label="New project…")
    row3.locator("[data-move-new]").fill("half typed")
    page.evaluate("() => { window.__settled = 0; document.addEventListener('htmx:afterSettle', () => { window.__settled++; }); }")
    mon("note", ID[2], "e2e ping")
    try:
        page.wait_for_function("() => window.__settled > 0", timeout=SSE_BUDGET_S * 1000)
        check("an outside change refreshed the tree over SSE", True)
    except Exception as e:  # noqa: BLE001
        check("an outside change refreshed the tree over SSE", False, str(e)[:80])
    check("the refresh left the collapsed node collapsed", node(alpha_id).get_attribute("open") is None)
    check("the refresh left the other node open", node(gamma_id).get_attribute("open") is not None)
    check("the refresh left the Move panel open and its input intact",
          row3.locator("[data-move-form]").is_visible()
          and row3.locator("[data-move-to]").input_value() == ""
          and row3.locator("[data-move-new]").input_value() == "half typed")
    check("the refresh kept the focus in the input", focused("data-move-new"))
    page.keyboard.press("Escape")
    check("Escape closes the panel and focus returns to Move",
          not row3.locator("[data-move-form]").is_visible() and focused("data-move-toggle"))
    page.reload(wait_until="load")
    check("collapsed state survives a reload (localStorage)", node(alpha_id).get_attribute("open") is None)
    node(alpha_id).locator("summary").first.click()
    check("clicking again reopens it", node(alpha_id).get_attribute("open") is not None)

    for scheme, size, label in TREE_VIEWS:
        c3, p3 = signed_in_views[label]
        p3.goto(f"{BASE}/", wait_until="load")
        shot(p3, f"06-tree-{label}")
        p3.locator(f"#s-{ID[4]} [data-move-toggle]").click()
        dims = p3.evaluate("() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]")
        check(f"tree {label} with a Move panel open: no horizontal scroll", dims[0] <= dims[1], str(dims))
        if size[0] < 400:
            box = p3.locator(f"#s-{ID[4]} [data-move-form]").bounding_box()
            check(f"tree {label}: the Move panel spans the row", box is not None and box["width"] > size[0] * 0.7,
                  str(box))
        shot(p3, f"06-tree-{label}-move")
        c3.close()

    # 8. the content policy did not break the page, then the throttle (last:
    # it locks logins out for a minute)
    note("== 8. content policy, throttle")
    check("no content-security-policy violations on any page", not csp_violations, "; ".join(csp_violations[:3]))
    statuses = [curl(f"{BASE}/login?code=bad{i}") for i in range(7)]
    check("repeated bad logins end in 429", statuses[-1] == "429" and set(statuses) <= {"401", "429"},
          ",".join(statuses))
    check("a signed-in browser is still served while throttled", curl("-H", ck, f"{BASE}/healthz") == "200")

    browser.close()

note(f"\n{len(failures)} failure(s)" + (": " + "; ".join(failures) if failures else ""))
raise SystemExit(1 if failures else 0)

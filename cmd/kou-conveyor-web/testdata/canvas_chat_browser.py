"""Looks at a kou agent's chat in its canvas node, in a real browser.

Run TestCanvasChatServe with KOU_CONVEYOR_CANVAS_CHAT_SERVE=<dir> first,
then: uv run --with playwright python canvas_chat_browser.py <dir> [label]
It writes screenshots and thinking.json (the computed style of the thinking
block in the ordinary chat and in the node) to <dir>, and exits non-zero
if what it checks does not hold.
"""
import json
import sys
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

out = Path(sys.argv[1])
label = sys.argv[2] if len(sys.argv) > 2 else "after"
read = lambda name: (out / name).read_text().strip()
canvas_url, chat_url = read("canvas"), read("chat")
# The session of the node none of whose runs began: no transcript yet.
fresh_sid = read("fresh")
# The toasts a page and its frames show, as they come (they go again).
TOASTS_JS = """(() => {
  window.__toasts = [];
  new MutationObserver((list) => {
    for (const m of list) for (const n of m.addedNodes) {
      if (n.nodeType === 1 && n.classList.contains('toast')) window.__toasts.push(n.className + ': ' + n.textContent);
    }
  }).observe(document, { childList: true, subtree: true });
})();"""
COULD_NOT = "could not stay open"
# The agents whose feed a page showed, by title, as it showed: a node to
# have its chat says the chat opens, not shows its feed a moment first.
FED_JS = """(() => {
  window.__fed = {};
  const look = () => {
    for (const n of document.querySelectorAll(".cv-node[data-kind='agent']")) {
      const feed = n.querySelector('.cv-agent-feed');
      if (!feed || feed.hidden || n.dataset.lod === 'low') continue;
      window.__fed[n.querySelector('.cv-title')?.textContent || n.dataset.id] ??= Math.round(performance.now());
    }
  };
  new MutationObserver(look).observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ['hidden', 'data-lod', 'data-mode'] });
})();"""
# While the cookie kc_no_sockets is there, no WebSocket opens — as with a
# server built before it took them, or a proxy that passes none: the page
# and its chats read their streams as EventSources.
NO_SOCKETS_JS = """(() => {
  const Real = window.WebSocket;
  window.WebSocket = class extends Real {
    constructor(...args) {
      if (document.cookie.split('; ').includes('kc_no_sockets=1')) throw new DOMException('no sockets here', 'SecurityError');
      super(...args);
    }
  };
})();"""
# The agent nodes in the board's view: what they show.
AGENTS_JS = """() => { const v = document.querySelector('.cv-viewport')?.getBoundingClientRect();
  return [...document.querySelectorAll(".cv-node[data-kind='agent']")].map((n) => {
    const r = n.getBoundingClientRect();
    const a = n.querySelector('.cv-agent');
    return { title: n.querySelector('.cv-title')?.textContent || '', lod: n.dataset.lod || '',
      inView: !!v && r.right > v.left && r.left < v.right && r.bottom > v.top && r.top < v.bottom,
      mode: a?.dataset.mode || '', chat: a?.dataset.chat || '' };
  }); }"""
STREAMS_JS = "async () => (await import('/kernel/events.js')).streams()"

PROPS = ["font-family", "font-size", "font-style", "line-height", "color", "border-left-width",
         "border-left-color", "padding-left", "margin-top", "background-color", "display", "opacity"]
STYLE_JS = """([sel, props]) => {
  const el = document.querySelector(sel);
  if (!el) return null;
  const out = {};
  const walk = [el, ...el.querySelectorAll('*')].slice(0, 12);
  walk.forEach((n, i) => { const cs = getComputedStyle(n); out[i + ':' + n.tagName + '.' + n.className] = Object.fromEntries(props.map((p) => [p, cs.getPropertyValue(p)])); });
  return out;
}"""
# The reasoning entry of the timeline.
THINK = "li.entry[data-kind='reasoning']"

failures = []
def dismiss(pg):
    # The gateway's update dialog, which a test server may offer.
    close = pg.locator("#gw-update:not([hidden]) button", has_text="Close")
    if close.count():
        close.first.click()
        pg.wait_for_timeout(200)
    # The agents' window, which shows by itself over the board's corner —
    # over the chat a click is meant for. Closed, it stays so for the tab.
    agents = pg.locator("section.cv-agents:not([hidden]) .cv-ag-head button", has_text="×")
    if agents.count():
        agents.first.click()
        pg.wait_for_timeout(200)

def check(ok, what):
    print(("ok   " if ok else "FAIL ") + what)
    if not ok:
        failures.append(what)

def agents(pg):
    return pg.evaluate(AGENTS_JS)

def in_view(pg):
    # The agents in view that show more than their header.
    return [a for a in agents(pg) if a["inView"] and a["lod"] != "low"]

def all_chats(pg, timeout=20000):
    # Every agent in view has its whole chat, ready: those agents, or None.
    for _ in range(timeout // 100):
        shown = in_view(pg)
        if shown and all(a["mode"] == "chat" and a["chat"] == "ready" for a in shown):
            return shown
        pg.wait_for_timeout(100)
    return None

def chats_shown(pg):
    return [a["title"] for a in agents(pg) if a["mode"] == "chat"]

with sync_playwright() as p:
    browser = p.chromium.launch()
    context = browser.new_context()
    # The gateway's update dialog, which the test server would ask the
    # module proxy for, is not what is looked at.
    context.route("**/api/gateway/update*", lambda route: route.fulfill(status=404, body='{"error":"not found"}', content_type="application/json"))
    browser.new_page = lambda **kw: (context.new_page(), context.pages[-1].set_viewport_size(kw["viewport"]) if "viewport" in kw else None)[0]
    page = browser.new_page(viewport={"width": 1440, "height": 900})
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))
    page.add_init_script(TOASTS_JS)
    page.add_init_script(FED_JS)
    # Reads of the session that has no transcript, which there is none of;
    # and of any session that is not there.
    missing, lost = [], []
    page.on("response", lambda r: missing.append(r.url) if fresh_sid in r.url and "/sessions/" in r.url and r.status == 404 else None)
    page.on("response", lambda r: lost.append(r.url) if "/sessions/" in r.url and r.status == 404 else None)
    def toasts(pg):
        # What the page and its frames toasted so far.
        seen = []
        for f in pg.frames:
            try:
                seen += f.evaluate("window.__toasts || []")
            except Exception:
                pass
        return seen

    # The ordinary chat: its thinking block, as the timeline draws it.
    page.goto(chat_url)
    page.wait_for_selector(THINK, timeout=15000)
    page.wait_for_timeout(500)
    think_sel = page.evaluate("""(sel) => { const el = document.querySelector(sel); if (!el) return null;
      return el.matches('[data-kind]') ? `[data-kind='${el.dataset.kind}']` : sel; }""", THINK)
    ordinary = page.evaluate(STYLE_JS, [THINK, PROPS])
    page.screenshot(path=str(out / f"{label}-ordinary.png"))
    check(ordinary is not None, "the ordinary chat shows the thinking block")

    if label == "before":
        (out / "thinking-before.json").write_text(json.dumps(ordinary, indent=1))
        browser.close()
        sys.exit(1 if failures else 0)

    # The canvas: its streams go over sockets, so each kou agent in view has
    # its whole chat — none its feed first, a moment, while the chat opens.
    page.goto(canvas_url)
    page.wait_for_selector(".cv-node[data-kind='agent']", timeout=15000)
    page.wait_for_timeout(800)
    dismiss(page)
    page.screenshot(path=str(out / f"{label}-canvas-initial.png"))
    nodes = page.locator(".cv-node[data-kind='agent']")
    check(nodes.count() == 3, "three agent nodes")
    node_of = lambda pg, title: pg.locator(f".cv-node:has(.cv-title:text-is('{title}'))")
    check(page.evaluate(STREAMS_JS) == "sockets", "the page reads its streams over sockets")
    shown = all_chats(page)
    check(shown is not None and len(shown) == 3, f"each agent in view has its whole chat {agents(page)}")
    fed = page.evaluate("window.__fed")
    check(not fed, f"no agent showed its feed before its chat {fed}")
    page.screenshot(path=str(out / f"{label}-canvas-chats.png"))

    # The node none of whose runs began: its session has an ID and no
    # transcript yet. Picked, it has the whole chat all the same — a new
    # session under that ID, its composer there for the first prompt — and
    # no error.
    fresh = node_of(page, "Fresh")
    check(fresh.count() == 1 and fresh.is_visible(), "the node that never ran is in view")
    fresh.locator(".cv-head, .cv-title").first.click()
    try:
        fresh.locator(".cv-agent[data-chat='ready'] iframe.cv-chat-frame").wait_for(timeout=20000)
    except Exception as e:
        print("no chat in the node that never ran:", str(e).splitlines()[0])
    asked = len(missing)
    # What took the chat away again — its page going back to its session
    # over and over, until the node gave up on it — has had its time.
    page.wait_for_timeout(2500)
    page.screenshot(path=str(out / f"{label}-canvas-fresh.png"))
    fresh.screenshot(path=str(out / f"{label}-node-fresh.png"))
    check(fresh.locator(".cv-agent[data-chat='ready'] iframe.cv-chat-frame").count() == 1, "never ran: the whole chat is in it")
    check(page.locator("iframe.cv-chat-frame").count() == 3, "never ran: the others keep their chats")
    ffr = [f for f in page.frames if "embed=canvas-chat" in f.url and fresh_sid in f.url]
    check(len(ffr) == 1 and ffr[0].locator(".dock textarea").count() >= 1, "never ran: with the chat's composer")
    check(len(ffr) == 1 and ffr[0].evaluate("location.hash").endswith("/s/" + fresh_sid), "never ran: the chat keeps to its session")
    check(fresh.locator(".cv-chat-hint:visible, .cv-chat-note:visible").count() == 0, "never ran: no note")
    print("reads of the session that is not there yet:", missing)
    check(1 <= asked <= 2 and len(missing) == asked, f"never ran: its session is asked for once, not over and over ({asked}, then {len(missing)})")
    said = [t for t in toasts(page) if COULD_NOT in t or "not found" in t.lower()]
    check(not said, f"never ran: no error {said}")

    first = node_of(page, "Helper")
    first.locator(".cv-head, .cv-title").first.click()
    first.locator(".cv-agent[data-chat='ready'] iframe.cv-chat-frame").wait_for(timeout=20000)
    page.wait_for_timeout(800)
    frames = page.locator("iframe.cv-chat-frame")
    check(frames.count() == 3, f"a chat in each agent (got {frames.count()})")
    frame = first.frame_locator("iframe.cv-chat-frame")
    page.screenshot(path=str(out / f"{label}-canvas-ready.png"))
    fr = [f for f in page.frames if "embed=canvas-chat" in f.url and read("session") in f.url][0]
    print("frame:", fr.url, fr.evaluate("location.hash + ' | ' + document.body.innerText.slice(0, 600).replace(/\\s+/g, ' ')"))
    frame.locator(THINK).first.wait_for(timeout=15000)
    embed_attr = fr.evaluate("document.documentElement.dataset.cvEmbed || ''")
    check(embed_attr == "chat", "the page in the frame is the chat")
    check(fr.evaluate("!document.querySelector('.cv-viewport, .cv-board')"), "no canvas inside the frame")
    check(fr.evaluate("getComputedStyle(document.querySelector('.rail')).display") == "none", "the rail is put away")
    check(fr.locator(".composer textarea, textarea#prompt, .dock textarea").count() >= 1, "the composer is there")
    check(fr.locator(THINK).count() >= 1, "the thinking block is there")
    check(fr.locator(".scroll").inner_text().count("echo:") >= 1, "the answers are there")
    # The timeline draws what is near the screen (timeline.js): the tool
    # call, above the brief's long echo, is drawn once scrolled to.
    try:
        fr.wait_for_function("""(() => { const s = document.querySelector('.scroll'); if (!s) return false;
          s.scrollTop = 0; return s.innerText.toLowerCase().includes('bash'); })""", timeout=5000)
        tool_shown = True
    except Exception:
        tool_shown = False
    check(tool_shown, "the tool call is there")
    fr.evaluate("(() => { const s = document.querySelector('.scroll'); s.scrollTop = s.scrollHeight; })()")
    page.wait_for_timeout(300)
    in_frame = fr.evaluate(STYLE_JS, [THINK, PROPS])
    # The ordinary chat in a window as wide as the frame: the timeline's
    # own narrow layout is the same either way.
    size = fr.evaluate("({ width: innerWidth, height: innerHeight })")
    narrow = browser.new_page(viewport=size)
    narrow.goto(chat_url)
    narrow.wait_for_selector(THINK, timeout=15000)
    narrow.wait_for_timeout(500)
    ordinary_narrow = narrow.evaluate(STYLE_JS, [THINK, PROPS])
    narrow.screenshot(path=str(out / f"{label}-ordinary-narrow.png"))
    narrow.close()
    print("frame size", size)
    check(in_frame == ordinary_narrow, "the thinking block looks the same in the node as in the ordinary chat as wide")
    if in_frame != ordinary_narrow:
        (out / "thinking-diff.json").write_text(json.dumps({"ordinary": ordinary_narrow, "frame": in_frame}, indent=1))
    before = out / "thinking-before.json"
    if before.exists():
        check(json.loads(before.read_text()) == ordinary, "the ordinary chat's thinking block is as before the change")
    # The thinking opens in the node as on the stage.
    fr.locator(THINK + " .fold").first.click()
    fr.locator(THINK + " .prose.thought").first.wait_for(timeout=5000)
    check(fr.locator(THINK + " .prose.thought").count() >= 1, "the thinking opens in the node")
    # The page around it keeps its address and its canvas.
    check("/c/board" in page.url, "the page stays on the canvas")
    check(page.evaluate("document.documentElement.dataset.cvEmbed") is None, "the page around it is no chat page")
    page.screenshot(path=str(out / f"{label}-canvas-chat.png"))
    first.screenshot(path=str(out / f"{label}-node.png"))

    # The keys: a click into the chat takes them, Shift+Esc gives them back.
    fr.locator(".dock textarea").first.click()
    page.wait_for_timeout(200)
    check(first.get_attribute("data-entered") == "true", "a click into the chat enters the node")
    fr.locator(".dock textarea").first.press("Shift+Escape")
    page.wait_for_timeout(200)
    check(first.get_attribute("data-entered") != "true", "Shift+Esc gives the keys back to the board")
    check(page.evaluate("document.activeElement?.tagName") != "IFRAME", "the board has the keys")

    # Picked, the second node has its chat, as it had; the first keeps its own.
    second = node_of(page, "Second")
    second.locator(".cv-head, .cv-title").first.click()
    page.wait_for_timeout(300)
    second.locator(".cv-agent[data-chat='ready']").wait_for(timeout=20000)
    check(first.locator(".cv-agent[data-mode='chat']").count() == 1, "the first node keeps its chat")
    page.screenshot(path=str(out / f"{label}-canvas-second.png"))

    # The theme, either way: switched on the page, every chat takes it; in a
    # chat, the page and the other chats take it. The page tells the frames
    # (they watch nothing of it).
    THEME = "document.documentElement.dataset.theme || ''"
    def themes():
        return [f.evaluate(THEME) for f in page.frames if "embed=canvas-chat" in f.url]
    def themed(want, tries=30):
        for _ in range(tries):
            if page.evaluate(THEME) == want and all(t == want for t in themes()):
                return True
            page.wait_for_timeout(100)
        return False
    was = page.evaluate(THEME)
    page.locator("#theme-toggle").click()
    now = page.evaluate(THEME)
    check(now in ("light", "dark") and themed(now), f"the theme switched on the page goes into every chat ({was} → {now}: {themes()})")
    page.screenshot(path=str(out / f"{label}-canvas-theme.png"))
    sfr = [f for f in page.frames if "embed=canvas-chat" in f.url and f.url != fr.url]
    if sfr:
        sfr[0].locator(".scroll").first.click(position={"x": 40, "y": 20})
        page.keyboard.press("t")
    back = "dark" if now == "light" else "light"
    check(bool(sfr) and themed(back), f"the theme switched in a chat goes to the page and the other chats ({page.evaluate(THEME)}: {themes()})")
    page.keyboard.press("Shift+Escape")
    page.wait_for_timeout(200)

    # What is written in the chat of the node that never ran is the first
    # prompt of its session: the session comes with it, and the node's agent
    # is told of the canvas first, as by a first message from the canvas.
    fresh.locator(".cv-head, .cv-title").first.click()
    try:
        fresh.locator(".cv-agent[data-chat='ready'] iframe.cv-chat-frame").wait_for(timeout=20000)
    except Exception as e:
        print("no chat in the node that never ran, picked again:", str(e).splitlines()[0])
    page.wait_for_timeout(300)
    ffr = [f for f in page.frames if "embed=canvas-chat" in f.url and fresh_sid in f.url]
    check(len(ffr) == 1, "picked again, the node that never ran has its chat again")
    if ffr:
        box = ffr[0].locator(".dock textarea").first
        box.click()
        box.fill("hi fresh")
        box.press("Enter")
    # The answer echoes the prompt as its agent was given it. Read as the
    # page's policy lets (wait_for_function evaluates a string, which it
    # forbids).
    def shows(text):
        for _ in range(150 if ffr else 0):
            if text in ffr[0].evaluate("() => document.querySelector('.scroll')?.innerText || ''"):
                return True
            page.wait_for_timeout(100)
        return False
    check(shows("echo: You are node «Fresh»"), "its first prompt, from its chat: the agent is told of the canvas with it")
    asked = len(missing)
    page.wait_for_timeout(1500)
    check(fresh.locator(".cv-agent[data-chat='ready'] iframe.cv-chat-frame").count() == 1, "its first run: the chat stays")
    check(len(chats_shown(page)) == 3, f"its first run: the others keep their chats {chats_shown(page)}")
    check(len(ffr) == 1 and ffr[0].evaluate("location.hash").endswith("/s/" + fresh_sid), "its first run: in its own session")
    check(page.evaluate("document.activeElement?.tagName") == "IFRAME", "its first run: the chat's composer has the keys")
    board = json.loads(urllib.request.urlopen(read("url") + "/api/canvases/board").read())
    runtime = next((n.get("runtime") or {} for n in board["doc"]["nodes"] if n.get("title") == "Fresh"), {})
    check(runtime.get("briefed") is True and runtime.get("session") == fresh_sid, f"its first run: the node knows its agent was told {runtime}")
    # The next prompt is as written.
    if ffr:
        box.fill("and again")
        box.press("Enter")
    check(shows("echo: and again"), "its next prompt, from its chat: answered")
    # Its title is the node's, as the canvas gave it, not its first prompt
    # (which starts with the brief).
    def titled(text):
        for _ in range(100 if ffr else 0):
            if ffr[0].evaluate("() => document.getElementById('crumb-title')?.textContent || ''") == text:
                return True
            page.wait_for_timeout(100)
        return False
    check(titled("Fresh"), "its runs: its chat is titled as the node is")
    check(len(missing) == asked, f"its runs: its session is not asked for again and again ({asked}, then {len(missing)})")
    fresh.screenshot(path=str(out / f"{label}-node-fresh-ran.png"))
    said = [t for t in toasts(page) if COULD_NOT in t or "not found" in t.lower()]
    check(not said, f"its first run: no error {said}")
    # It is out of focus — Shift+Esc gives the keys back — for what follows.
    page.keyboard.press("Shift+Escape")
    page.wait_for_timeout(200)

    # A kou agent the user adds (+ Add) has the whole chat at once, unpicked,
    # its composer there for the first prompt, and no error.
    page.locator("button.cv-add").first.click()
    page.locator("#menu button[role='menuitem']:has(.l:text-is('kou agent'))").first.click()
    added = node_of(page, "Agent")
    try:
        added.locator(".cv-agent[data-chat='ready'] iframe.cv-chat-frame").wait_for(timeout=20000)
    except Exception as e:
        print("no chat in the agent just added:", str(e).splitlines()[0])
    page.wait_for_timeout(1500)
    page.screenshot(path=str(out / f"{label}-canvas-added.png"))
    if added.count():
        added.screenshot(path=str(out / f"{label}-node-added.png"))
    check(page.locator(".cv-node[data-kind='agent']").count() == 4 and added.count() == 1, "added: a fourth agent")
    check(added.locator(".cv-agent[data-chat='ready'] iframe.cv-chat-frame").count() == 1, "added: the whole chat is in it")
    check(all_chats(page, 5000) is not None, f"added: each agent in view has its chat {agents(page)}")
    board = json.loads(urllib.request.urlopen(read("url") + "/api/canvases/board").read())
    added_sid = next(((n.get("runtime") or {}).get("session", "") for n in board["doc"]["nodes"] if n.get("title") == "Agent"), "")
    afr = [f for f in page.frames if "embed=canvas-chat" in f.url and added_sid and added_sid in f.url]
    check(len(afr) == 1 and afr[0].locator(".dock textarea").count() >= 1, f"added: with the chat's composer ({added_sid})")
    check(len(afr) == 1 and afr[0].evaluate("location.hash").endswith("/s/" + added_sid), "added: the chat keeps to its session")
    check(added.locator(".cv-chat-hint:visible, .cv-chat-note:visible").count() == 0, "added: no note")
    reads = [u for u in lost if added_sid and added_sid in u]
    check(1 <= len(reads) <= 2, f"added: its session is asked for once, not over and over ({len(reads)})")
    said = [t for t in toasts(page) if COULD_NOT in t or "not found" in t.lower() or "not added" in t.lower()]
    check(not said, f"added: no error {said}")

    # Panned away and back, zoomed out and in again, within a few seconds —
    # as one looks around a board — the chats are as they were: kept, not
    # loaded anew (each would be the whole cockpit loading).
    loads = []
    page.on("request", lambda r: loads.append(r.url) if r.resource_type == "document" and "embed=canvas-chat" in r.url else None)
    PAN_JS = "(dx) => document.querySelector('.cv-viewport').dispatchEvent(new WheelEvent('wheel', { deltaX: dx, bubbles: true, cancelable: true }))"
    ZOOM_JS = """(dy) => { const v = document.querySelector('.cv-viewport'); const r = v.getBoundingClientRect();
      v.dispatchEvent(new WheelEvent('wheel', { deltaY: dy, ctrlKey: true, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2, bubbles: true, cancelable: true })); }"""
    KEEP_FOR = 10000
    def pan(dx, wait=400):
        page.evaluate(PAN_JS, dx)
        page.wait_for_timeout(wait)
    def zoom_out():
        steps = 0
        while in_view(page) and steps < 12:
            page.evaluate(ZOOM_JS, 60)
            page.wait_for_timeout(300)
            steps += 1
        return steps
    def zoom_in(steps):
        for _ in range(steps):
            page.evaluate(ZOOM_JS, -60)
            page.wait_for_timeout(100)
    chats = len(all_chats(page) or [])
    check(chats >= 3, f"the agents in view have their chats ({chats})")
    page.evaluate("window.__fed = {}")
    pan(6000, 1500)
    check(not in_view(page), "panned away: no agent in view")
    pan(-6000, 0)
    back = all_chats(page, 5000)
    check(back is not None and len(back) == chats and not loads, f"panned back within seconds: the chats as they were, none loaded anew ({len(loads)})")
    steps = zoom_out()
    page.wait_for_timeout(800)
    check(not in_view(page) and page.locator("iframe.cv-chat-frame:visible").count() == 0, f"zoomed out: no chat shows ({steps} steps)")
    zoom_in(steps)
    back = all_chats(page, 5000)
    check(back is not None and len(back) == chats and not loads, f"zoomed in again: the chats as they were, none loaded anew ({len(loads)})")
    page.screenshot(path=str(out / f"{label}-canvas-kept.png"))
    kfr = [f for f in page.frames if "embed=canvas-chat" in f.url]
    shown_size = kfr[0].evaluate("[innerWidth, innerHeight]") if kfr else [0, 0]
    check(shown_size[0] > 100 and shown_size[1] > 100, f"a chat kept, shown again, is laid out ({shown_size})")
    # Gone longer, they are let go, and open anew once back.
    pan(6000, KEEP_FOR + 1500)
    check(page.locator("iframe.cv-chat-frame").count() == 0, "away longer: the chats kept are let go")
    pan(-6000, 0)
    back = all_chats(page)
    check(back is not None and len(back) == chats and len(loads) == chats, f"back after that: the chats open anew ({len(loads)})")
    # Meanwhile none showed its feed — back in view before the board looked
    # again, a node shows its chat, kept, or that it opens.
    fed = page.evaluate("window.__fed")
    check(not fed, f"panned and zoomed away and back: no agent showed its feed meanwhile {fed}")

    # What is let go is gone: chats let go and opened anew, the canvas left
    # and opened again, the page holds no more documents than before. (A
    # chat whose page the page around it held — an observer of its theme —
    # stayed, the whole cockpit, each time.)
    cdp = context.new_cdp_session(page)
    def documents(pg, session):
        for _ in range(3):
            session.send("HeapProfiler.collectGarbage")
            pg.wait_for_timeout(300)
        return session.send("Memory.getDOMCounters")["documents"]
    status = page.evaluate("""async () => (await fetch('/api/canvases', { method: 'POST',
      headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id: 'other', title: 'Other' }) })).status""")
    check(status in (200, 201), f"another canvas ({status})")
    board_hash = page.evaluate("location.hash")
    def elsewhere_and_back():
        page.evaluate("(h) => { location.hash = h; }", board_hash.replace("/c/board", "/c/other"))
        page.wait_for_timeout(1500)
        left = page.locator("iframe.cv-chat-frame").count() == 0 and not agents(page)
        page.evaluate("(h) => { location.hash = h; }", board_hash)
        page.wait_for_timeout(500)
        return all_chats(page) if left else None
    elsewhere_and_back()
    before = documents(page, cdp)
    for _ in range(3):
        back = elsewhere_and_back()
    check(back is not None and len(back) == chats, f"the canvas opened again: its chats ({agents(page)})")
    pan(6000, KEEP_FOR + 1500)
    pan(-6000, 0)
    back = all_chats(page)
    after = documents(page, cdp)
    print(f"documents: {before} before, {after} after")
    check(back is not None and after <= before, f"the chats let go are gone: no more documents than before ({before} → {after})")
    cdp.detach()

    # A selection dragged from the transcript onto the composer, or onto the
    # activity line just over it, ends at the transcript's last character
    # once the button is let go: what the agent writes next does not come
    # into it (it did, and so went on growing, its end in the dock). And it
    # keeps its ends as the entries they are in are drawn again, as a run
    # starts and ends (they went to the list, and the selection with them).
    sel = browser.new_page(viewport={"width": 1440, "height": 900})
    sel.on("pageerror", lambda e: errors.append(str(e)))
    sel.goto(chat_url)
    sel.wait_for_selector("li.entry[data-kind='assistant']", timeout=15000)
    sel.wait_for_timeout(800)
    dismiss(sel)
    SELECTED_JS = """() => { const s = getSelection(); const list = document.getElementById('entries');
      if (!s.rangeCount || s.isCollapsed) return { text: '', inList: false };
      const r = s.getRangeAt(0);
      return { text: s.toString(), inList: list.contains(r.startContainer) && list.contains(r.endContainer) }; }"""
    # Where a drag starts: in the first line of a text near the end, a
    # whole pixel into it (from a character's middle, a synthetic drag
    # moves a caret instead of selecting).
    START_JS = """() => { const box = document.getElementById('scroll').getBoundingClientRect();
      const shown = [...document.querySelectorAll("li.entry .prose > p, li.entry[data-kind='user'] .text")].filter((t) => {
        const b = t.getBoundingClientRect(); return b.width > 80 && b.top >= box.top + 8 && b.top + 18 <= box.bottom - 8; });
      const el = shown[Math.max(0, shown.length - 2)];
      const r = document.createRange(); r.selectNodeContents(el);
      const line = r.getClientRects()[0];
      return [Math.round(line.left + Math.min(25, line.width / 3)), Math.round(line.top + line.height / 2)]; }"""
    def center(selector):
        return sel.evaluate("""(s) => { const b = document.querySelector(s).getBoundingClientRect();
          return [Math.round(b.left + b.width / 2), Math.round(b.top + b.height / 2)]; }""", selector)
    def drag(start, end):
        sel.evaluate("getSelection().removeAllRanges()")
        sel.mouse.move(*start)
        sel.mouse.down()
        for i in range(1, 13):
            sel.mouse.move(start[0] + (end[0] - start[0]) * i / 12, start[1] + (end[1] - start[1]) * i / 12)
            sel.wait_for_timeout(20)
        sel.mouse.up()
        sel.wait_for_timeout(200)
        return sel.evaluate(SELECTED_JS)
    def start_run(prompt):
        # From elsewhere, as another front-end or a node starts one: the
        # pointer stays where it was let go.
        return sel.evaluate("""async ([sid, prompt]) => (await fetch('/api/runs', { method: 'POST',
          headers: { 'content-type': 'application/json' }, body: JSON.stringify({ prompt, session_id: sid }) })).status""", [read("session"), prompt])
    def shown_in(selector, text, tries=150):
        for _ in range(tries):
            if text in sel.evaluate("(s) => document.querySelector(s)?.innerText || ''", selector):
                return True
            sel.wait_for_timeout(100)
        return False
    def activity_shown(tries=100):
        for _ in range(tries):
            if sel.evaluate("() => { const a = document.getElementById('activity'); return !!a && !a.hidden && a.getBoundingClientRect().height > 0; }"):
                return True
            sel.wait_for_timeout(100)
        return False
    def kept(before, after):
        # The same ends: between them, an entry may show other actions as a
        # run goes and ends, but no other entries.
        return bool(after["text"]) and after["inList"] and after["text"][:12] == before["text"][:12] and after["text"][-12:] == before["text"][-12:]
    let_go = drag(sel.evaluate(START_JS), center("#prompt"))
    check(bool(let_go["text"]) and let_go["inList"], f"a selection dragged onto the composer ends in the transcript once let go {let_go}")
    check(start_run("slow select one") == 202, "a run starts from elsewhere")
    check(shown_in("#scroll", "echo: slow select one"), "its answer comes")
    later = sel.evaluate(SELECTED_JS)
    check("echo: slow select one" not in later["text"], f"the answer that came after does not come into the selection {later}")
    check(kept(let_go, later), f"the selection keeps its ends as the run starts and ends {later}")
    sel.screenshot(path=str(out / f"{label}-selection-composer.png"))
    # A run held at its gate (a file in the workspace) until the drag is
    # over, started while it goes on: the entries are drawn again as the run
    # starts, and its activity line, its clock ticking, shows over the
    # composer, where the drag ends; the run's answer comes only once the
    # gate is there.
    ANCHOR_JS = """() => { const s = getSelection(); const n = s.anchorNode;
      return { node: !n ? '' : n.nodeType === 3 ? n.data : n.nodeName, offset: s.anchorOffset }; }"""
    ws_id = chat_url.split("#/w/", 1)[1].split("/", 1)[0]
    workspaces = json.loads(urllib.request.urlopen(read("url") + "/api/workspaces").read())
    gate = Path(next(w["path"] for w in workspaces if w["id"] == ws_id)) / "gate"
    gate.unlink(missing_ok=True)
    start = sel.evaluate(START_JS)
    sel.evaluate("getSelection().removeAllRanges()")
    sel.mouse.move(*start)
    sel.mouse.down()
    for i in range(1, 5):
        sel.mouse.move(start[0] + 10 * i, start[1] + 4 * i)
        sel.wait_for_timeout(20)
    began = sel.evaluate(ANCHOR_JS)
    started = 0
    for _ in range(100):
        # The run before ends a moment after its answer shows.
        started = start_run("gate select two")
        if started != 409:
            break
        sel.wait_for_timeout(100)
    running = started == 202 and activity_shown(200)
    check(running, f"a run, its activity line over the composer ({started})")
    sel.wait_for_timeout(300)
    still = sel.evaluate(ANCHOR_JS)
    check(bool(began["node"]) and still == began, f"a drag going on as a run starts keeps where it started {began} {still}")
    end = center("#activity") if running else [start[0] + 40, start[1] + 16]
    for i in range(1, 13):
        sel.mouse.move(start[0] + 40 + (end[0] - start[0] - 40) * i / 12, start[1] + 16 + (end[1] - start[1] - 16) * i / 12)
        sel.wait_for_timeout(20)
    sel.mouse.up()
    sel.wait_for_timeout(200)
    if running:
        let_go = sel.evaluate(SELECTED_JS)
        check(bool(let_go["text"]) and let_go["inList"] and "Esc" not in let_go["text"], f"a selection dragged onto the activity line ends in the transcript once let go {let_go}")
        sel.wait_for_timeout(1500)
        ticked = sel.evaluate(SELECTED_JS)
        check(kept(let_go, ticked), f"the clock of the activity line ticking does not move it {ticked}")
        gate.touch()
        check(shown_in("#scroll", "echo: gate select two"), "the run's answer comes")
        later = sel.evaluate(SELECTED_JS)
        check("echo: gate select two" not in later["text"] and "Esc" not in later["text"], f"the answer does not come into the selection, nor the activity line {later}")
        check(kept(let_go, later), f"the selection keeps its ends as the run ends {later}")
        sel.screenshot(path=str(out / f"{label}-selection-activity.png"))
    gate.unlink(missing_ok=True)
    sel.close()

    # The chat page by itself, framed by no one, is the ordinary cockpit.
    alone = browser.new_page(viewport={"width": 1200, "height": 800})
    alone.goto(read("url") + "/?embed=canvas-chat&ws=x&session=y#/")
    alone.wait_for_timeout(1500)
    check(alone.evaluate("document.documentElement.dataset.cvEmbed") is None, "not framed: not a chat page")
    alone.close()
    page.close()

    # A page whose streams cannot go over sockets (a proxy that passes none)
    # reads them as EventSources, each holding one of the six connections a
    # browser has to a server over HTTP/1.1: one chat at a time then — the
    # one used last has it — and none kept.
    ev = browser.new_page(viewport={"width": 1440, "height": 900})
    ev_errors = []
    ev.on("pageerror", lambda e: ev_errors.append(str(e)))
    ev.add_init_script(NO_SOCKETS_JS)
    ev.add_init_script(FED_JS)
    context.add_cookies([{"name": "kc_no_sockets", "value": "1", "url": read("url")}])
    ev.goto(canvas_url)
    ev.wait_for_selector(".cv-agent[data-chat='ready']", timeout=20000)
    ev.wait_for_timeout(800)
    dismiss(ev)
    check(ev.evaluate(STREAMS_JS) == "events", "no sockets: the page reads its streams as EventSources")
    one = chats_shown(ev)
    check(len(one) == 1, f"no sockets: one chat at a time {agents(ev)}")
    fed = ev.evaluate("window.__fed")
    check(len(one) == 1 and one[0] not in fed, f"no sockets: the agent with the chat did not show its feed first {fed}")
    helper_ev, second_ev = node_of(ev, "Helper"), node_of(ev, "Second")
    def take(node):
        node.locator(".cv-head").first.click()
        try:
            node.locator(".cv-agent[data-mode='chat'][data-chat='ready']").wait_for(timeout=20000)
        except Exception as e:
            print("no chat:", str(e).splitlines()[0])
        ev.wait_for_timeout(300)
    take(helper_ev)
    take(second_ev)
    check(chats_shown(ev) == ["Second"], f"no sockets: picked, another takes the chat {agents(ev)}")
    check(ev.locator("iframe.cv-chat-frame").count() == 1, "no sockets: the chat it had is not kept")
    check(helper_ev.locator(".cv-agent-feed").is_visible() and helper_ev.locator(".cv-chat-hint").is_visible(), "no sockets: the one that had it shows its feed, and the way back to its chat")
    ev.screenshot(path=str(out / f"{label}-canvas-events.png"))
    # The chats let go there are gone too.
    ev_cdp = context.new_cdp_session(ev)
    before = documents(ev, ev_cdp)
    for _ in range(3):
        take(helper_ev)
        take(second_ev)
    after = documents(ev, ev_cdp)
    print(f"no sockets, documents: {before} before, {after} after")
    check(chats_shown(ev) == ["Second"] and after <= before, f"no sockets: the chats let go are gone ({before} → {after})")
    ev_cdp.detach()
    # Panned away and back within seconds, its one chat is kept there too:
    # no other shows or opens meanwhile.
    ev_loads = []
    ev.on("request", lambda r: ev_loads.append(r.url) if r.resource_type == "document" and "embed=canvas-chat" in r.url else None)
    ev.evaluate(PAN_JS, 6000)
    ev.wait_for_timeout(1500)
    away = ev.locator("iframe.cv-chat-frame").count()
    ev.evaluate(PAN_JS, -6000)
    back = False
    for _ in range(50):
        if chats_shown(ev) == ["Second"] and second_ev.locator(".cv-agent[data-chat='ready']").count():
            back = True
            break
        ev.wait_for_timeout(100)
    check(away == 1 and back and not ev_loads, f"no sockets: panned away and back within seconds, its chat is kept ({away} kept, {len(ev_loads)} loaded)")
    # Focus mode (F) on one, then another picked: that one has the chat. (The
    # one in focus mode kept it, whatever was picked.)
    take(helper_ev)
    ev.keyboard.press("f")
    ev.wait_for_timeout(800)
    check(chats_shown(ev) == ["Helper"], f"focus mode: the node in it has the chat {agents(ev)}")
    take(second_ev)
    check(chats_shown(ev) == ["Second"], f"after focus mode: the node picked has the chat {agents(ev)}")
    # The server goes and comes back as one that takes sockets (a build of
    # it with them, taken up): the page's streams go over them, with no
    # reload, and every agent in view has its chat.
    ev.evaluate("window.__same = true")
    context.clear_cookies(name="kc_no_sockets")
    (out / "drop").write_text("")
    for _ in range(200):
        if ev.evaluate(STREAMS_JS) == "sockets":
            break
        ev.wait_for_timeout(100)
    how = ev.evaluate(STREAMS_JS)
    check(how == "sockets" and ev.evaluate("window.__same === true"), f"the server back with sockets: the page reads its streams over them, with no reload ({how})")
    shown = all_chats(ev)
    check(shown is not None and len(shown) >= 2, f"the server back with sockets: each agent in view has its chat {agents(ev)}")
    check(not ev_errors, f"no sockets: no page errors {ev_errors}")
    ev.close()

    # A server that does not let the chat be framed: the nodes keep their
    # feed and say why; once the server is back as one that lets it, the
    # chat comes, with no reload.
    old = browser.new_page(viewport={"width": 1440, "height": 900})
    old_errors = []
    old.on("pageerror", lambda e: old_errors.append(e.stack or str(e)))
    def deny(route):
        res = route.fetch()
        headers = {**res.headers, "content-security-policy": "frame-ancestors 'none'", "x-frame-options": "DENY"}
        route.fulfill(response=res, headers=headers)
    old.route("**/?embed=canvas-chat*", deny)
    old.goto(canvas_url)
    old.wait_for_selector(".cv-node[data-kind='agent']", timeout=15000)
    old.wait_for_timeout(800)
    dismiss(old)
    node_of(old, "Helper").locator(".cv-head").click()
    old.wait_for_timeout(1500)
    check(old.locator("iframe.cv-chat-frame").count() == 0, "framing denied: no chat frame")
    check(old.locator(".cv-node[data-kind='agent'] .cv-agent-feed").first.is_visible(), "framing denied: the feed shows")
    check(old.locator(".cv-chat-hint:visible").count() == 0, "framing denied: no way to a chat that cannot open")
    check(old.locator(".cv-chat-note:visible").count() >= 1, "framing denied: the node says why")
    old.screenshot(path=str(out / f"{label}-canvas-denied.png"))
    node_of(old, "Helper").screenshot(path=str(out / f"{label}-node-denied.png"))
    check(not old_errors, f"framing denied: no page errors {old_errors}")
    # The server goes and comes back (its event streams break), now as one
    # that lets the chat be framed.
    asks = []
    old.on("request", lambda r: asks.append(r.url) if "embed=canvas-chat" in r.url and r.resource_type == "fetch" else None)
    old.unroute("**/?embed=canvas-chat*")
    (out / "drop").write_text("")
    try:
        old.locator(".cv-agent[data-chat='ready'] iframe.cv-chat-frame").first.wait_for(timeout=20000)
    except Exception as e:
        print("no chat after the server came back:", str(e).splitlines()[0])
    check(len(asks) == 1, f"the server asked once again whether it lets the chat be framed ({len(asks)})")
    check(all_chats(old) is not None, f"the server back as one that lets it: the chats come, with no reload {agents(old)}")
    check(old.locator(".cv-chat-note:visible").count() == 0, "and no node says it cannot")
    old.screenshot(path=str(out / f"{label}-canvas-back.png"))
    check(not old_errors, f"the server back: no page errors {old_errors}")

    check(not errors, f"no page errors {errors}")
    browser.close()

print("FAILED" if failures else "PASSED", len(failures))
sys.exit(1 if failures else 0)

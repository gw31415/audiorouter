#!/usr/bin/env python3
"""Record audiorouter dashboard interactions via Chrome DevTools Protocol.

Creates assets/dashboard.gif from real browser screenshots while sending real
mouse events: drag node, drag source handle to target handle, select route,
open config/log.
"""
from __future__ import annotations

import base64
import json
import os
import shutil
import socket
import struct
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
FRAMES = ROOT / "assets" / "dashboard-recording-frames"
OUT_GIF = ROOT / "assets" / "dashboard.gif"
URL = "http://127.0.0.1:7823"
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
PORT = 9333
VIEW_W = 1600
VIEW_H = 1000


def read_http_json(url: str, timeout: float = 10.0):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        try:
            with urllib.request.urlopen(url, timeout=1) as r:
                return json.loads(r.read().decode())
        except Exception as e:
            last = e
            time.sleep(0.1)
    raise RuntimeError(f"failed to read {url}: {last}")


class WS:
    def __init__(self, ws_url: str):
        assert ws_url.startswith("ws://")
        host_port, path = ws_url[5:].split("/", 1)
        host, port = host_port.split(":")
        self.sock = socket.create_connection((host, int(port)), timeout=5)
        key = base64.b64encode(os.urandom(16)).decode()
        req = (
            f"GET /{path} HTTP/1.1\r\n"
            f"Host: {host_port}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(req.encode())
        resp = self.sock.recv(4096)
        if b" 101 " not in resp:
            raise RuntimeError(resp.decode(errors="replace"))
        self.next_id = 1

    def _send_text(self, text: str):
        payload = text.encode()
        header = bytearray([0x81])
        n = len(payload)
        if n < 126:
            header.append(0x80 | n)
        elif n < 65536:
            header.append(0x80 | 126)
            header += struct.pack("!H", n)
        else:
            header.append(0x80 | 127)
            header += struct.pack("!Q", n)
        mask = os.urandom(4)
        header += mask
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(header + masked)

    def _recv_text(self) -> str:
        def recv_exact(n: int) -> bytes:
            chunks = []
            got = 0
            while got < n:
                c = self.sock.recv(n - got)
                if not c:
                    raise EOFError("websocket closed")
                chunks.append(c)
                got += len(c)
            return b"".join(chunks)

        b1, b2 = recv_exact(2)
        opcode = b1 & 0x0F
        masked = bool(b2 & 0x80)
        length = b2 & 0x7F
        if length == 126:
            length = struct.unpack("!H", recv_exact(2))[0]
        elif length == 127:
            length = struct.unpack("!Q", recv_exact(8))[0]
        mask = recv_exact(4) if masked else b""
        data = recv_exact(length)
        if masked:
            data = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        if opcode == 8:
            raise EOFError("websocket close frame")
        if opcode in (1, 0):
            return data.decode()
        return self._recv_text()

    def call(self, method: str, params: dict | None = None):
        msg_id = self.next_id
        self.next_id += 1
        self._send_text(json.dumps({"id": msg_id, "method": method, "params": params or {}}))
        while True:
            msg = json.loads(self._recv_text())
            if msg.get("id") == msg_id:
                if "error" in msg:
                    raise RuntimeError(msg["error"])
                return msg.get("result")


def js_string(expr: str) -> str:
    return expr


def eval_js(cdp: WS, expr: str, await_promise: bool = False):
    res = cdp.call(
        "Runtime.evaluate",
        {
            "expression": expr,
            "returnByValue": True,
            "awaitPromise": await_promise,
        },
    )
    if "exceptionDetails" in res:
        raise RuntimeError(res["exceptionDetails"])
    return res.get("result", {}).get("value")


def mouse(cdp: WS, typ: str, x: float, y: float, button: str = "left", buttons: int = 0):
    cdp.call(
        "Input.dispatchMouseEvent",
        {"type": typ, "x": x, "y": y, "button": button, "buttons": buttons, "pointerType": "mouse"},
    )


def click(cdp: WS, x: float, y: float):
    mouse(cdp, "mouseMoved", x, y)
    mouse(cdp, "mousePressed", x, y, buttons=1)
    mouse(cdp, "mouseReleased", x, y, buttons=0)
    time.sleep(0.25)


def ease(t: float) -> float:
    """Smoothstep easing for less robotic cursor movement."""
    return t * t * (3 - 2 * t)


def drag(cdp: WS, start, end, steps: int = 24, pause: float = 0.008, capture=None):
    sx, sy = start
    ex, ey = end
    mouse(cdp, "mouseMoved", sx, sy)
    time.sleep(0.08)
    mouse(cdp, "mousePressed", sx, sy, buttons=1)
    for i in range(1, steps + 1):
        t = ease(i / steps)
        x = sx + (ex - sx) * t
        y = sy + (ey - sy) * t
        mouse(cdp, "mouseMoved", x, y, buttons=1)
        if capture and (i % 2 == 0 or i == steps):
            capture((x, y))
        time.sleep(pause)
    mouse(cdp, "mouseReleased", ex, ey, buttons=0)
    time.sleep(0.18)


def move_cursor(cdp: WS, start, end, steps: int = 12, pause: float = 0.006, capture=None):
    sx, sy = start
    ex, ey = end
    for i in range(1, steps + 1):
        t = ease(i / steps)
        x = sx + (ex - sx) * t
        y = sy + (ey - sy) * t
        mouse(cdp, "mouseMoved", x, y)
        if capture and (i % 3 == 0 or i == steps):
            capture((x, y))
        time.sleep(pause)
    return (ex, ey)


def screenshot(cdp: WS, path: Path):
    data = cdp.call("Page.captureScreenshot", {"format": "png", "fromSurface": True})["data"]
    path.write_bytes(base64.b64decode(data))


def frame_with_cursor(src: Path, dst: Path, pos=None, label: str | None = None):
    im = Image.open(src).convert("RGBA")
    draw = ImageDraw.Draw(im)
    # Keep README GIF clean: do not draw explanatory label boxes over the UI.
    if pos:
        x, y = pos
        # simple white cursor arrow with dark outline
        pts = [(x, y), (x, y + 26), (x + 7, y + 20), (x + 12, y + 34), (x + 18, y + 32), (x + 13, y + 18), (x + 23, y + 18)]
        draw.polygon(pts, fill=(255, 255, 255, 255), outline=(0, 0, 0, 255))
    im.save(dst)


def capture_labeled(cdp: WS, idx: int, label: str, cursor=None) -> Path:
    raw = FRAMES / f"raw-{idx:03d}.png"
    out = FRAMES / f"frame-{idx:03d}.png"
    screenshot(cdp, raw)
    frame_with_cursor(raw, out, cursor, label)
    return out


def get_rects(cdp: WS):
    return eval_js(
        cdp,
        r"""
(() => {
  const box = (el) => { const r = el.getBoundingClientRect(); return {x:r.x, y:r.y, w:r.width, h:r.height, cx:r.x+r.width/2, cy:r.y+r.height/2, text:el.innerText||''}; };
  const nodes = [...document.querySelectorAll('.react-flow__node')].map(box);
  const handles = [...document.querySelectorAll('.react-flow__handle')].map(box);
  const buttons = [...document.querySelectorAll('button')].map(box);
  return {nodes, handles, buttons};
})()
""",
    )


def wait_for(cdp: WS, predicate: str, timeout: float = 8.0):
    end = time.time() + timeout
    while time.time() < end:
        if eval_js(cdp, predicate):
            return
        time.sleep(0.1)
    raise TimeoutError(predicate)


def footer_tab(cdp: WS, label: str):
    """Click a footer tab by visible text and return its screen center."""
    return eval_js(
        cdp,
        f"""
(() => {{
  const buttons = [...document.querySelectorAll('button')];
  const btn = buttons.find((b) => (b.innerText || '').trim().includes({label!r}));
  if (!btn) throw new Error('footer tab not found: {label}');
  const r = btn.getBoundingClientRect();
  btn.click();
  return {{cx:r.x + r.width / 2, cy:r.y + r.height / 2}};
}})()
""",
    )


def bottom_panel_open(cdp: WS) -> bool:
    return bool(eval_js(cdp, "document.querySelector('section.h-64') !== null"))


def fit_view_button(cdp: WS):
    """Return the center of React Flow's Fit View control button."""
    return eval_js(
        cdp,
        """
(() => {
  const btn = document.querySelector('.react-flow__controls-fitview')
    || [...document.querySelectorAll('button')].find((b) => (b.getAttribute('title') || '').includes('Fit View'));
  if (!btn) throw new Error('Fit View button not found');
  const r = btn.getBoundingClientRect();
  return {cx:r.x + r.width / 2, cy:r.y + r.height / 2};
})()
""",
    )


def click_fit_view(cdp: WS):
    """Click Fit View through the actual React Flow control element."""
    return eval_js(
        cdp,
        """
(() => {
  const btn = document.querySelector('.react-flow__controls-fitview')
    || [...document.querySelectorAll('button')].find((b) => (b.getAttribute('title') || '').includes('Fit View'));
  if (!btn) throw new Error('Fit View button not found');
  btn.dispatchEvent(new MouseEvent('mousedown', {bubbles:true, cancelable:true, buttons:1}));
  btn.dispatchEvent(new MouseEvent('mouseup', {bubbles:true, cancelable:true}));
  btn.click();
  return true;
})()
""",
    )


def main():
    if FRAMES.exists():
        shutil.rmtree(FRAMES)
    FRAMES.mkdir(parents=True)
    user_data = tempfile.mkdtemp(prefix="audiorouter-chrome-")
    chrome = subprocess.Popen(
        [
            CHROME,
            "--headless=new",
            f"--remote-debugging-port={PORT}",
            f"--user-data-dir={user_data}",
            f"--window-size={VIEW_W},{VIEW_H}",
            "--hide-scrollbars",
            "about:blank",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        targets = read_http_json(f"http://127.0.0.1:{PORT}/json/list")
        page = next((t for t in targets if t.get("type") == "page"), None)
        if page is None:
            raise RuntimeError(f"no page target: {targets}")
        cdp = WS(page["webSocketDebuggerUrl"])
        cdp.call("Page.enable")
        cdp.call("Runtime.enable")
        cdp.call("Emulation.setDeviceMetricsOverride", {"width": VIEW_W, "height": VIEW_H, "deviceScaleFactor": 1, "mobile": False})
        cdp.call("Page.navigate", {"url": URL})
        wait_for(cdp, "document.querySelector('input[type=checkbox]') !== null")
        eval_js(cdp, "document.querySelector('input[type=checkbox]').click(); true")
        wait_for(cdp, "document.querySelectorAll('.react-flow__node').length >= 2")
        time.sleep(0.8)

        frames = []
        long_frames: set[int] = set()
        cursor = (760, 500)
        frames.append(capture_labeled(cdp, 1, "Dashboard: devices loaded from config", cursor))

        rects = get_rects(cdp)
        mic = next(n for n in rects["nodes"] if "mic" in n["text"])
        out = next(n for n in rects["nodes"] if "out" in n["text"])
        cursor = move_cursor(
            cdp,
            cursor,
            (out["cx"], out["cy"]),
            capture=lambda pos: frames.append(capture_labeled(cdp, len(frames) + 1, "Move to out node", pos)),
        )
        frames.append(capture_labeled(cdp, len(frames)+1, "Drag the out node to arrange the graph", cursor))
        drag(
            cdp,
            (out["cx"], out["cy"]),
            (out["cx"] + 180, out["cy"] + 90),
            steps=22,
            pause=0.007,
            capture=lambda pos: frames.append(capture_labeled(cdp, len(frames) + 1, "Dragging out node", pos)),
        )
        cursor = (out["cx"] + 180, out["cy"] + 90)
        frames.append(capture_labeled(cdp, len(frames)+1, "Node position updated", cursor))

        rects = get_rects(cdp)
        mic = next(n for n in rects["nodes"] if "mic" in n["text"])
        out = next(n for n in rects["nodes"] if "out" in n["text"])
        handles = rects["handles"]
        # Route direction is explicit: mic's right/source handle -> out's left/target handle.
        source = min(
            [h for h in handles if h["cx"] > mic["cx"] and mic["y"] - 10 <= h["cy"] <= mic["y"] + mic["h"] + 10],
            key=lambda h: abs(h["cx"] - (mic["x"] + mic["w"])),
        )
        target = min(
            [h for h in handles if h["cx"] < out["cx"] and out["y"] - 10 <= h["cy"] <= out["y"] + out["h"] + 10],
            key=lambda h: abs(h["cx"] - out["x"]),
        )
        cursor = move_cursor(
            cdp,
            cursor,
            (source["cx"], source["cy"]),
            capture=lambda pos: frames.append(capture_labeled(cdp, len(frames) + 1, "Move to mic source handle", pos)),
        )
        frames.append(capture_labeled(cdp, len(frames)+1, "Connect mic → out: drag source handle to target handle", cursor))
        drag(
            cdp,
            (source["cx"], source["cy"]),
            (target["cx"], target["cy"]),
            steps=30,
            pause=0.007,
            capture=lambda pos: frames.append(capture_labeled(cdp, len(frames) + 1, "Drawing route: mic → out", pos)),
        )
        wait_for(cdp, "document.querySelectorAll('.react-flow__edge').length >= 1")
        route = eval_js(cdp, """
(() => {
  const e = document.querySelector('.react-flow__edge');
  return e ? {source:e.getAttribute('data-id') || '', text: document.body.innerText.includes('mic') && document.body.innerText.includes('out')} : null;
})()
""")
        cursor = (target["cx"], target["cy"])
        long_frames.add(len(frames))
        frames.append(capture_labeled(cdp, len(frames)+1, "Route connected: mic → out", cursor))
        time.sleep(0.8)
        long_frames.add(len(frames))
        frames.append(capture_labeled(cdp, len(frames)+1, "Pause after connecting the route", None))

        # Show device Inspector before route Inspector. Dispatch a real bubbling
        # click on the React Flow node, then wait until the side panel updates.
        mic_click = eval_js(cdp, r"""
(() => {
  const node = [...document.querySelectorAll('.react-flow__node')].find((n) => (n.innerText || '').includes('mic'));
  if (!node) throw new Error('mic node not found');
  const r = node.getBoundingClientRect();
  const x = r.x + r.width / 2;
  const y = r.y + r.height / 2;
  node.dispatchEvent(new MouseEvent('click', {bubbles:true, cancelable:true, clientX:x, clientY:y}));
  return {cx:x, cy:y};
})()
""")
        cursor = move_cursor(
            cdp,
            cursor,
            (mic_click["cx"], mic_click["cy"]),
            capture=lambda pos: frames.append(capture_labeled(cdp, len(frames) + 1, "Move to mic node", pos)),
        )
        wait_for(cdp, "[...document.querySelectorAll('aside h2')].some((h) => h.innerText.includes('デバイス設定'))")
        long_frames.add(len(frames))
        frames.append(capture_labeled(cdp, len(frames)+1, "Device Inspector: mic", cursor))

        # Select route edge for route Inspector, and wait for the route panel.
        edge_click = eval_js(cdp, r"""
(() => {
  const edge = document.querySelector('.react-flow__edge');
  if (!edge) throw new Error('route edge not found');
  const r = edge.getBoundingClientRect();
  const x = r.x + r.width / 2;
  const y = r.y + r.height / 2;
  edge.dispatchEvent(new MouseEvent('click', {bubbles:true, cancelable:true, clientX:x, clientY:y}));
  return {cx:x, cy:y};
})()
""")
        cursor = move_cursor(
            cdp,
            cursor,
            (edge_click["cx"], edge_click["cy"]),
            capture=lambda pos: frames.append(capture_labeled(cdp, len(frames) + 1, "Move to route", pos)),
        )
        wait_for(cdp, "[...document.querySelectorAll('aside h2')].some((h) => h.innerText.includes('ルート設定'))")
        long_frames.add(len(frames))
        frames.append(capture_labeled(cdp, len(frames)+1, "Route Inspector: channels, gain, mute", cursor))

        # Adjust gain by dragging the route midpoint label upward, then click it to mute.
        gain = eval_js(cdp, r"""
(() => {
  const el = document.querySelector('.cursor-ns-resize');
  if (!el) throw new Error('gain label not found');
  const r = el.getBoundingClientRect();
  return {cx:r.x + r.width / 2, cy:r.y + r.height / 2};
})()
""")
        cursor = move_cursor(
            cdp,
            cursor,
            (gain["cx"], gain["cy"]),
            capture=lambda pos: frames.append(capture_labeled(cdp, len(frames) + 1, "Move to gain control", pos)),
        )
        drag(
            cdp,
            cursor,
            (gain["cx"], gain["cy"] - 48),
            steps=20,
            pause=0.007,
            capture=lambda pos: frames.append(capture_labeled(cdp, len(frames) + 1, "Drag gain up", pos)),
        )
        cursor = (gain["cx"], gain["cy"] - 48)
        wait_for(cdp, "document.body.innerText.includes('+') && document.body.innerText.includes('dB')")
        long_frames.add(len(frames))
        frames.append(capture_labeled(cdp, len(frames)+1, "Gain adjusted by vertical drag", cursor))

        gain2 = eval_js(cdp, r"""
(() => {
  const el = document.querySelector('.cursor-ns-resize');
  const r = el.getBoundingClientRect();
  return {cx:r.x + r.width / 2, cy:r.y + r.height / 2};
})()
""")
        cursor = move_cursor(
            cdp,
            cursor,
            (gain2["cx"], gain2["cy"]),
            capture=lambda pos: frames.append(capture_labeled(cdp, len(frames) + 1, "Move to mute toggle", pos)),
        )
        click(cdp, gain2["cx"], gain2["cy"])
        cursor = (gain2["cx"], gain2["cy"])
        wait_for(cdp, "document.body.innerText.includes('✕')")
        long_frames.add(len(frames))
        frames.append(capture_labeled(cdp, len(frames)+1, "Mute toggled by clicking route control", cursor))

        # Switch footer tabs without closing the bottom panel.
        tab_frames: set[int] = set()

        valid_pos_preview = eval_js(cdp, """
(() => { const btn = [...document.querySelectorAll('button')].find((b) => (b.innerText || '').trim().includes('valid')); const r = btn.getBoundingClientRect(); return {cx:r.x+r.width/2, cy:r.y+r.height/2}; })()
""")
        cursor = move_cursor(cdp, cursor, (valid_pos_preview["cx"], valid_pos_preview["cy"]), capture=lambda pos: frames.append(capture_labeled(cdp, len(frames)+1, "Move to validation tab", pos)))
        valid_pos = footer_tab(cdp, "valid")
        wait_for(cdp, "document.querySelector('section.h-64') !== null")
        tab_frames.add(len(frames))
        frames.append(capture_labeled(cdp, len(frames)+1, "Footer tab: validation", cursor))

        # Fit View immediately after opening the footer tab.
        fit_pos = fit_view_button(cdp)
        cursor = move_cursor(cdp, cursor, (fit_pos["cx"], fit_pos["cy"]), capture=lambda pos: frames.append(capture_labeled(cdp, len(frames)+1, "Move to Fit View", pos)))
        click_fit_view(cdp)
        cursor = (fit_pos["cx"], fit_pos["cy"])
        long_frames.add(len(frames))
        frames.append(capture_labeled(cdp, len(frames)+1, "Fit View after validation tab opens", cursor))

        config_pos_preview = eval_js(cdp, """
(() => { const btn = [...document.querySelectorAll('button')].find((b) => (b.innerText || '').trim().includes('config.toml')); const r = btn.getBoundingClientRect(); return {cx:r.x+r.width/2, cy:r.y+r.height/2}; })()
""")
        cursor = move_cursor(cdp, cursor, (config_pos_preview["cx"], config_pos_preview["cy"]), capture=lambda pos: frames.append(capture_labeled(cdp, len(frames)+1, "Move to config tab", pos)))
        config_pos = footer_tab(cdp, "config.toml")
        wait_for(cdp, "document.querySelector('section.h-64') !== null")
        tab_frames.add(len(frames))
        frames.append(capture_labeled(cdp, len(frames)+1, "Footer tab: config.toml preview", cursor))

        # Fit View immediately after switching to the TOML tab as well.
        fit_pos = fit_view_button(cdp)
        cursor = move_cursor(cdp, cursor, (fit_pos["cx"], fit_pos["cy"]), capture=lambda pos: frames.append(capture_labeled(cdp, len(frames)+1, "Move to Fit View", pos)))
        click_fit_view(cdp)
        cursor = (fit_pos["cx"], fit_pos["cy"])
        long_frames.add(len(frames))
        frames.append(capture_labeled(cdp, len(frames)+1, "Fit View after config tab opens", cursor))

        images = [Image.open(p).convert("P", palette=Image.Palette.ADAPTIVE, colors=256) for p in frames]
        durations = [55] * len(images)
        for i in long_frames:
            durations[i] = 1200
        for i in tab_frames:
            durations[i] = 2200
        for i in [0, 1, len(images) - 1]:
            durations[i] = 900
        images[0].save(OUT_GIF, save_all=True, append_images=images[1:], duration=durations, loop=0, optimize=True)
        print(f"wrote {OUT_GIF} with {len(images)} frames")
    finally:
        chrome.terminate()
        try:
            chrome.wait(timeout=3)
        except subprocess.TimeoutExpired:
            chrome.kill()
        shutil.rmtree(user_data, ignore_errors=True)


if __name__ == "__main__":
    main()

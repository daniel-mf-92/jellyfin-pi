#!/usr/bin/env python3
"""JMP guard: inject QWebChannel polyfill via CDP, auto-login, CSS cleanup.

The Jellyfin server's web client doesn't include Qt's qwebchannel.js,
so native input (d-pad, controller) never reaches the web UI.
This script injects QWebChannel via CDP, connects it to the Qt transport,
and wires up window.api so nativeshell.js works properly.
"""

import json
import os
import time
import urllib.request
from pathlib import Path

try:
    import websocket
except Exception:
    raise SystemExit(0)

# Load .env from script directory
_env_file = Path(__file__).resolve().parent / ".env"
if _env_file.exists():
    for _line in _env_file.read_text().splitlines():
        _line = _line.strip()
        if _line and not _line.startswith("#") and "=" in _line:
            _k, _, _v = _line.partition("=")
            _k, _v = _k.strip(), _v.strip().strip("\"'")
            if _k and _k not in os.environ:
                os.environ[_k] = _v

SERVER_BASE = os.environ.get("JELLYFIN_URL", "http://localhost:8096")
USER = os.environ.get("JELLYFIN_USER", "")
PASSWORD = os.environ.get("JELLYFIN_PASS", "")
CDP_JSON_URL = "http://127.0.0.1:9222/json"

# Minified Qt5 QWebChannel + QObject — standard implementation
QWEBCHANNEL_INJECT = r"""
(function() {
if (typeof window.QWebChannel !== "undefined") return "already_defined";
var T={signal:1,propertyUpdate:2,init:3,idle:4,debug:5,invokeMethod:6,connectToSignal:7,disconnectFromSignal:8,setProperty:9,response:10};
window.QWebChannel=function(transport,initCallback){
    if(typeof transport!=="object"||typeof transport.send!=="function"){console.error("QWebChannel: bad transport");return}
    var ch=this;ch.transport=transport;
    ch.send=function(d){if(typeof d!=="string")d=JSON.stringify(d);ch.transport.send(d)};
    ch.transport.onmessage=function(m){var d=m.data;if(typeof d==="string")d=JSON.parse(d);
    switch(d.type){case T.signal:ch.handleSignal(d);break;case T.response:ch.handleResponse(d);break;case T.propertyUpdate:ch.handlePropertyUpdate(d);break;default:console.error("invalid msg",m.data)}};
    ch.execCallbacks={};ch.execId=0;
    ch.exec=function(d,cb){if(!cb){ch.send(d);return}if(ch.execId===Number.MAX_VALUE)ch.execId=Number.MIN_VALUE;d.id=ch.execId++;ch.execCallbacks[d.id]=cb;ch.send(d)};
    ch.objects={};
    ch.handleSignal=function(m){var o=ch.objects[m.object];if(o)o.signalEmitted(m.signal,m.args);else console.warn("Unhandled signal")};
    ch.handleResponse=function(m){if(!m.hasOwnProperty("id")){return}ch.execCallbacks[m.id](m.data);delete ch.execCallbacks[m.id]};
    ch.handlePropertyUpdate=function(m){m.data.forEach(function(d){var o=ch.objects[d.object];if(o)o.propertyUpdate(d.signals,d.properties)});ch.exec({type:T.idle})};
    ch.exec({type:T.init},function(data){for(var n in data)new window.__QObj(n,data[n],ch);if(initCallback)initCallback(ch);ch.exec({type:T.idle})})
};
window.__QObj=function(name,data,wc){
    this.__id__=name;wc.objects[name]=this;this.__objectSignals__={};this.__propertyCache__={};var o=this;
    o.unwrapQObject=function(r){if(r instanceof Array){var ret=[];for(var i=0;i<r.length;i++)ret[i]=o.unwrapQObject(r[i]);return ret}if(!(r instanceof Object))return r;if(!r["__QObject*__"]||r.id===undefined){var j={};for(var p in r)j[p]=o.unwrapQObject(r[p]);return j}var oid=r.id;if(wc.objects[oid])return wc.objects[oid];if(!r.data)return;var q=new window.__QObj(oid,r.data,wc);try{q.destroyed.connect(function(){if(wc.objects[oid]===q)delete wc.objects[oid]})}catch(e){}q.unwrapProperties();return q};
    o.unwrapProperties=function(){for(var i in o.__propertyCache__)o.__propertyCache__[i]=o.unwrapQObject(o.__propertyCache__[i])};
    function addSignal(sd,isPN){var sn=sd[0],si=sd[1];o[sn]={connect:function(cb){if(typeof cb!=="function")return;o.__objectSignals__[si]=o.__objectSignals__[si]||[];o.__objectSignals__[si].push(cb);if(!isPN&&sn!=="destroyed")wc.exec({type:T.connectToSignal,object:o.__id__,signal:si})},disconnect:function(cb){if(typeof cb!=="function")return;o.__objectSignals__[si]=o.__objectSignals__[si]||[];var idx=o.__objectSignals__[si].indexOf(cb);if(idx!==-1)o.__objectSignals__[si].splice(idx,1);if(!isPN&&o.__objectSignals__[si].length===0)wc.exec({type:T.disconnectFromSignal,object:o.__id__,signal:si})}}}
    function addMethod(md){var mn=md[0],mi=md[1];o[mn]=function(){var args=[],cb;for(var i=0;i<arguments.length;i++){var a=arguments[i];if(typeof a==="function")cb=a;else if(a instanceof window.__QObj&&wc.objects[a.__id__]!==undefined)args.push({id:a.__id__});else args.push(a)}wc.exec({type:T.invokeMethod,object:o.__id__,method:mi,args:args},function(r){if(r!==undefined){var res=o.unwrapQObject(r);if(cb)cb(res)}})}}
    function bindGS(pi){var px=pi[0],pn=pi[1],ns=pi[2];o.__propertyCache__[px]=pi[3];if(ns&&ns[0]===1)addSignal(ns,true);Object.defineProperty(o,pn,{configurable:true,get:function(){return o.__propertyCache__[px]},set:function(v){if(v===undefined)return;o.__propertyCache__[px]=v;var vs=v;if(vs instanceof window.__QObj&&wc.objects[vs.__id__]!==undefined)vs={id:vs.__id__};wc.exec({type:T.setProperty,object:o.__id__,property:px,value:vs})}})}
    data.methods.forEach(addMethod);data.properties.forEach(bindGS);data.signals.forEach(function(s){addSignal(s,false)});Object.assign(o,o.__propertyCache__);
    o.propertyUpdate=function(sigs,pm){for(var pi in pm)o.__propertyCache__[pi]=o.unwrapQObject(pm[pi]);for(var sn in sigs)o.signalEmitted(sigs[sn],[o.__propertyCache__[sn]])};
    o.signalEmitted=function(sn,sa){var c=o.__objectSignals__[sn];if(c)c.forEach(function(cb){cb.apply(cb,sa)})}
};
return "injected";
})()
"""

# After injecting QWebChannel class, connect it and wire up window.api
CONNECT_AND_INIT = r"""
new Promise(function(resolve, reject) {
    try {
        if (window.api && window.api.input) {
            resolve("api_already_set");
            return;
        }
        new window.QWebChannel(window.qt.webChannelTransport, function(channel) {
            window.api = channel.objects;
            // Wire up settings sync (from nativeshell.js logic)
            try {
                if (window.api.settings && window.api.settings.sectionValueUpdate) {
                    window.api.settings.sectionValueUpdate.connect(function(section, key, value) {
                        try {
                            var raw = JSON.parse(window.sessionStorage.getItem("settings") || "{}");
                            if (!raw[section]) raw[section] = {};
                            raw[section][key] = value;
                            window.sessionStorage.setItem("settings", JSON.stringify(raw));
                        } catch(e) {}
                    });
                }
            } catch(e) {}
            resolve("connected:" + Object.keys(channel.objects).join(","));
        });
        setTimeout(function() { resolve("timeout"); }, 5000);
    } catch(e) { resolve("error:" + e.message); }
})
"""

CSS = """
*, *:focus, *:focus-visible, *:focus-within, *:active, *:hover {
  outline: none !important; box-shadow: none !important;
}
.navMenuOption, .navMenuOption-selected, .button-flat,
.paper-icon-button-light, .emby-button, .cardBox, .itemAction,
.mdc-button, .mdc-icon-button, .mdc-tab, .mdc-card,
button, a, input, select, textarea {
  outline: none !important; box-shadow: none !important;
}
""".strip()


def get_page(timeout=1.0):
    try:
        with urllib.request.urlopen(CDP_JSON_URL, timeout=timeout) as resp:
            pages = json.loads(resp.read().decode())
    except Exception:
        return None
    for page in (pages or []):
        if page.get("type") == "page" and page.get("webSocketDebuggerUrl"):
            return page
    return (pages[0] if pages and pages[0].get("webSocketDebuggerUrl") else None)


def connect_ws(max_wait=10.0):
    deadline = time.time() + max_wait
    while time.time() < deadline:
        page = get_page(timeout=0.8)
        if page:
            try:
                ws = websocket.create_connection(
                    page["webSocketDebuggerUrl"], suppress_origin=True, timeout=3.0)
                ws.settimeout(3.0)
                return ws
            except Exception:
                pass
        time.sleep(0.3)
    return None


ws = connect_ws()
if not ws:
    raise SystemExit(0)

message_id = 0


def cdp(method, params=None, timeout_s=5.0):
    global message_id
    message_id += 1
    payload = {"id": message_id, "method": method}
    if params:
        payload["params"] = params
    try:
        ws.send(json.dumps(payload))
    except Exception:
        return None
    deadline = time.time() + timeout_s
    while time.time() < deadline:
        try:
            reply = json.loads(ws.recv())
        except Exception:
            return None
        if reply.get("id") == message_id:
            return reply
    return None


def js(expression, await_promise=False):
    reply = cdp(
        "Runtime.evaluate",
        {"expression": expression, "awaitPromise": await_promise, "returnByValue": True},
        timeout_s=8.0,
    )
    if not reply:
        return None
    return ((reply.get("result") or {}).get("result") or {}).get("value")


# Step 1: Inject QWebChannel class
result = js(QWEBCHANNEL_INJECT)
print(f"QWebChannel inject: {result}")

# Step 2: Connect to Qt transport and wire up window.api
result = js(CONNECT_AND_INIT, await_promise=True)
print(f"QWebChannel connect: {result}")

# Step 3: Auto-login if needed
time.sleep(0.5)
current_url = str(js("window.location.href") or "")
needs_login = bool(js("Boolean(document.querySelector('input[type=password]'))")) or "#/login" in current_url

if needs_login:
    js(f"""
(() => {{
  const user = {json.dumps(USER)};
  const cards = Array.from(document.querySelectorAll('.card, .userCard, [data-type="User"]'));
  for (const card of cards) {{
    if ((card.textContent || '').includes(user)) {{ card.click(); break; }}
  }}
  return true;
}})();
""")
    time.sleep(0.5)
    js(f"""
(() => {{
  const pass = {json.dumps(PASSWORD)};
  const input = document.querySelector('input[type=password]');
  if (input) {{
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(input, pass);
    input.dispatchEvent(new Event('input', {{ bubbles: true }}));
  }}
  const remember = document.querySelector('input[type=checkbox]');
  if (remember && !remember.checked) remember.click();
  const buttons = Array.from(document.querySelectorAll('button'));
  for (const b of buttons) {{
    const t = (b.textContent || '').trim().toLowerCase();
    if (t === 'sign in' || t === 'login') {{ b.click(); break; }}
  }}
  return true;
}})();
""")
    print("Auto-login attempted")

# Step 4: Inject CSS
js(f"""
(() => {{
  let s = document.getElementById('pi5-no-outline-style');
  if (!s) {{ s = document.createElement('style'); s.id = 'pi5-no-outline-style'; (document.head || document.documentElement).appendChild(s); }}
  s.textContent = {json.dumps(CSS)};
  if (!window.__pi5CssTimer) {{
    window.__pi5CssTimer = setInterval(() => {{
      try {{
        let s = document.getElementById('pi5-no-outline-style');
        if (!s) {{ s = document.createElement('style'); s.id = 'pi5-no-outline-style'; (document.head || document.documentElement).appendChild(s); }}
        s.textContent = {json.dumps(CSS)};
      }} catch(_) {{}}
    }}, 5000);
  }}
  return true;
}})();
""")

try:
    ws.close()
except Exception:
    pass

/* CleanRSTAI browser client. Plain script, no modules, no dependencies.
 * Drives index.html and style.css through the JSON API (spec section 4).
 * Untrusted text only goes in through textContent. The one innerHTML use is
 * CleanMarkdown.render, which escapes everything before adding markup. */
(function () {
  "use strict";

  // Phosphor codepoints from style.css (see the table at its top).
  var ICON = {
    PLUS: "", STOP: "", PIN: "", UNPIN: "", TRASH: "",
    EDIT: "", REGEN: "", TOOL: ""
  };
  var PLAIN_HTTP_TEXT = "Plain HTTP: the password and chats cross the network unencrypted.";
  var PHONE = window.matchMedia("(max-width: 760px)");
  var DARK = window.matchMedia("(prefers-color-scheme: dark)");
  var MAC = /Mac|iPhone|iPad/.test(navigator.platform || "");

  var dom = {
    login: document.getElementById("login"),
    loginForm: document.getElementById("login-form"),
    loginPassword: document.getElementById("login-password"),
    loginSubmit: document.getElementById("login-submit"),
    loginError: document.getElementById("login-error"),
    plainWarning: document.getElementById("plain-http-warning"),
    app: document.getElementById("app"),
    backdrop: document.getElementById("sidebar-backdrop"),
    sidebarToggle: document.getElementById("sidebar-toggle"),
    newChat: document.getElementById("new-chat"),
    search: document.getElementById("chat-search"),
    chatList: document.getElementById("chat-list"),
    logout: document.getElementById("logout"),
    title: document.getElementById("chat-title"),
    banner: document.getElementById("banner"),
    messages: document.getElementById("messages"),
    greeting: document.getElementById("greeting"),
    form: document.getElementById("composer-form"),
    input: document.getElementById("input"),
    stop: document.getElementById("stop"),
    send: document.getElementById("send"),
    providerSelect: document.getElementById("provider-select"),
    modelSelect: document.getElementById("model-select"),
    palette: document.getElementById("palette"),
    paletteInput: document.getElementById("palette-input"),
    paletteList: document.getElementById("palette-list")
  };

  var state = {
    loggedIn: false,
    plainHttp: false,
    data: null,            // last StateView
    openId: null,          // open chat id, or null for the empty new-chat view
    chat: null,            // last ChatView for openId
    renderedId: "__none__",
    optimistic: null,      // { chat, text, base }: a sent message not yet in the chat view
    sending: false,
    transient: null,       // { text } error shown for 6 s
    transientTimer: null,
    chatError: null,       // ChatView.error, shown until the chat clears it
    menuFor: null,         // chat id whose row menu is open
    palItems: [],
    palIndex: 0,
    palFocus: null,
    pickerSig: "",
    restoreKeys: new Set(),
    es: null,
    retryTimer: null,
    retryDelay: 1000
  };

  // Rendered Markdown per assistant message of the open chat, keyed by chat, index and role.
  // An entry is reused only while its content is unchanged. Cleared when the open chat changes.
  var MD_CACHE_MAX = 500;
  var mdCache = new Map();

  // Refetches of the open chat run at most once per CHAT_THROTTLE_MS. A trailing call is always scheduled.
  var CHAT_THROTTLE_MS = 250;
  var chatThrottle = { at: 0, timer: null };

  // ---- Small helpers --------------------------------------------------------

  // el("tag", {cls, text, attrs, on, data}, [children]): children are nodes or strings.
  function el(tag, props, kids) {
    var node = document.createElement(tag);
    if (props) {
      if (props.cls) node.className = props.cls;
      if (props.text != null) node.textContent = props.text;
      if (props.attrs) Object.keys(props.attrs).forEach(function (k) { node.setAttribute(k, props.attrs[k]); });
      if (props.on) Object.keys(props.on).forEach(function (k) { node.addEventListener(k, props.on[k]); });
      if (props.data) Object.keys(props.data).forEach(function (k) { node.dataset[k] = props.data[k]; });
    }
    (kids || []).forEach(function (k) {
      if (k) node.appendChild(typeof k === "string" ? document.createTextNode(k) : k);
    });
    return node;
  }

  function icon(cp) {
    return el("span", { cls: "icon", text: cp, attrs: { "aria-hidden": "true" } });
  }

  function listen(mq, fn) {
    if (mq.addEventListener) mq.addEventListener("change", fn);
    else mq.addListener(fn);
  }

  function isEditable(t) {
    return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT" || t.isContentEditable);
  }

  function visibleCount(list) {
    return (list || []).filter(function (m) { return m.role !== "system"; }).length;
  }

  function prettyJson(s) {
    if (s == null || s === "") return "";
    if (typeof s !== "string") return JSON.stringify(s, null, 2);
    try { return JSON.stringify(JSON.parse(s), null, 2); } catch (_) { return s; }
  }

  function isPhone() { return PHONE.matches; }

  // Runs fn at most once at a time; calls made meanwhile collapse into one queued run.
  function coalesce(fn) {
    var busy = false;
    var again = false;
    return function () {
      if (busy) { again = true; return; }
      busy = true;
      (async function () {
        do {
          again = false;
          try { await fn(); } catch (_) { /* api() has already shown the error */ }
        } while (again);
        busy = false;
      })();
    };
  }

  function sendsOnEnter() {
    return state.data ? !!state.data.send_with_enter : true;
  }

  function greetingText() {
    var h = new Date().getHours();
    var part = h < 12 ? "Good morning" : h < 18 ? "Good afternoon" : "Good evening";
    var name = state.data && state.data.user_name;
    return name ? part + ", " + name : part;
  }

  function parseHashId() {
    var m = /^#\/chat\/([0-9a-f-]{36})$/i.exec(location.hash);
    return m ? m[1] : null;
  }

  function setHash(id, replace) {
    var want = id ? "#/chat/" + id : "";
    if (location.hash === want) return;
    if (replace || !id) history.replaceState(null, "", location.pathname + location.search + want);
    else location.hash = want;
  }

  // ---- API ------------------------------------------------------------------

  // Non-GET requests carry X-CleanRSTAI (CSRF guard). Errors reject with {status, error}.
  // quiet: the caller shows the error itself. Other errors go to the banner for 6 s.
  async function api(method, path, body, quiet) {
    var opts = { method: method, credentials: "same-origin", headers: {} };
    if (method !== "GET") opts.headers["X-CleanRSTAI"] = "1";
    if (body !== undefined) {
      opts.headers["Content-Type"] = "application/json";
      opts.body = JSON.stringify(body);
    }
    var res;
    try {
      res = await fetch(path, opts);
    } catch (_) {
      var offline = { status: 0, error: "Could not reach the server" };
      if (!quiet) flash(offline.error);
      throw offline;
    }
    var text = await res.text();
    var data = {};
    try { data = text ? JSON.parse(text) : {}; } catch (_) { data = {}; }
    if (!res.ok) {
      var err = { status: res.status, error: data.error || res.statusText || ("HTTP " + res.status) };
      if (res.status === 401 && path !== "/api/login") onLoggedOut();
      else if (!quiet) flash(err.error);
      throw err;
    }
    return data;
  }

  function onLoggedOut() {
    if (state.loggedIn) showLogin();
  }

  // ---- Banner ---------------------------------------------------------------

  // Priority: a transient error, then the open chat's error, then the plain-HTTP notice.
  function paintBanner() {
    var text = null;
    var warn = false;
    if (state.transient) {
      text = state.transient.text;
    } else if (state.chatError) {
      text = state.chatError;
    } else if (state.plainHttp && state.loggedIn) {
      text = PLAIN_HTTP_TEXT;
      warn = true;
    }
    dom.banner.textContent = text || "";
    dom.banner.classList.toggle("warning", warn);
    dom.banner.hidden = text == null;
  }

  function flash(text) {
    state.transient = { text: text };
    clearTimeout(state.transientTimer);
    state.transientTimer = setTimeout(function () {
      state.transient = null;
      paintBanner();
    }, 6000);
    paintBanner();
  }

  // ---- Screens --------------------------------------------------------------

  function showLogin() {
    state.loggedIn = false;
    closeStream();
    closeMenus();
    dom.palette.hidden = true;
    state.data = null;
    state.openId = null;
    state.chat = null;
    state.optimistic = null;
    state.transient = null;
    state.chatError = null;
    state.renderedId = "__none__";
    paintBanner();
    dom.app.hidden = true;
    dom.login.hidden = false;
    dom.plainWarning.hidden = !state.plainHttp;
    dom.loginError.textContent = "";
    history.replaceState(null, "", location.pathname + location.search);
    dom.loginPassword.focus();
  }

  function enterApp() {
    state.loggedIn = true;
    dom.login.hidden = true;
    dom.app.hidden = false;
    dom.loginPassword.value = "";
    paintBanner();
    start();
  }

  async function start() {
    connectStream();
    try {
      await loadState();
    } catch (_) {
      return;
    }
    var target = parseHashId() || newestChatId();
    openChat(target, { replace: true });
  }

  function newestChatId() {
    var best = null;
    ((state.data && state.data.chats) || []).forEach(function (c) {
      if (!best || Date.parse(c.updated_at) > Date.parse(best.updated_at)) best = c;
    });
    return best ? best.id : null;
  }

  // ---- State, theme, sidebar, model picker ------------------------------------

  async function loadState() {
    state.data = await api("GET", "/api/state");
    applyTheme();
    renderSidebar();
    renderPicker();
  }

  var refreshState = coalesce(loadState);

  // "system" follows prefers-color-scheme (listened to below).
  function applyTheme() {
    var t = state.data ? state.data.theme : "system";
    var dark = t === "dark" || (t !== "light" && DARK.matches);
    document.documentElement.dataset.theme = dark ? "dark" : "light";
  }

  function toggleSidebar() {
    dom.app.classList.toggle(isPhone() ? "sidebar-open" : "sidebar-collapsed");
    syncSidebarToggle();
  }

  function sidebarOpen() {
    return isPhone() ? dom.app.classList.contains("sidebar-open") : !dom.app.classList.contains("sidebar-collapsed");
  }

  function syncSidebarToggle() {
    dom.sidebarToggle.setAttribute("aria-expanded", String(sidebarOpen()));
  }

  function closeSidebarOnPhone() {
    if (isPhone()) {
      dom.app.classList.remove("sidebar-open");
      syncSidebarToggle();
    }
  }

  function renderSidebar() {
    if (!state.data) return;
    var q = dom.search.value.trim().toLowerCase();
    var chats = state.data.chats.filter(function (c) {
      return !q || (c.title || "").toLowerCase().indexOf(q) !== -1;
    });
    var pinned = chats.filter(function (c) { return c.pinned; });
    var recent = chats.filter(function (c) { return !c.pinned; });
    var nodes = [];
    if (pinned.length) {
      nodes.push(el("div", { cls: "chat-group", text: "Pinned" }));
      pinned.forEach(function (c) { nodes.push(chatRow(c)); });
    }
    if (recent.length) {
      nodes.push(el("div", { cls: "chat-group", text: "Recent" }));
      recent.forEach(function (c) { nodes.push(chatRow(c)); });
    }
    if (!nodes.length) {
      nodes.push(el("div", { cls: "chat-group", text: q ? "No matching chats" : "No chats yet" }));
    }
    dom.chatList.replaceChildren.apply(dom.chatList, nodes);

    // Keep an open row menu open across a re-render.
    if (state.menuFor) {
      var id = state.menuFor;
      state.menuFor = null;
      var row = dom.chatList.querySelector('.chat-row[data-id="' + id + '"]');
      var chat = state.data.chats.find(function (c) { return c.id === id; });
      if (row && chat) openMenu(row, chat, true);
    }
  }

  function chatRow(c) {
    var active = c.id === state.openId;
    var title = c.title || "Untitled chat";
    var row = el("div", {
      cls: "chat-row" + (active ? " active" : "") + (c.running ? " running" : ""),
      data: { id: c.id }
    });
    var open = el("button", {
      cls: "chat-open", text: title, attrs: { type: "button" },
      on: { click: function () { openChat(c.id); } }
    });
    if (active) open.setAttribute("aria-current", "true");
    var badge = el("span", { cls: "chat-badge", text: c.approvals ? String(c.approvals) : "" });
    var menu = el("button", {
      cls: "chat-menu", text: "…",
      attrs: { type: "button", "aria-label": "Options for " + title, "aria-haspopup": "menu", "aria-expanded": "false" },
      on: { click: function (e) { e.stopPropagation(); toggleMenu(row, c); } }
    });
    row.appendChild(open);
    row.appendChild(badge);
    row.appendChild(menu);
    return row;
  }

  function toggleMenu(row, c) {
    if (state.menuFor === c.id) closeMenus();
    else openMenu(row, c, false);
  }

  function openMenu(row, c, keepFocus) {
    closeMenus();
    var items = [
      menuItem(ICON.EDIT, "Rename", function () { renameChat(c); }),
      menuItem(c.pinned ? ICON.UNPIN : ICON.PIN, c.pinned ? "Unpin" : "Pin", function () { pinChat(c); }),
      menuItem(ICON.TRASH, "Delete", function () { deleteChat(c); }, true)
    ];
    row.appendChild(el("div", { cls: "menu", attrs: { role: "menu" } }, items));
    row.querySelector(".chat-menu").setAttribute("aria-expanded", "true");
    state.menuFor = c.id;
    if (!keepFocus) items[0].focus();
  }

  function menuItem(cp, label, fn, danger) {
    return el("button", {
      cls: danger ? "danger" : "",
      attrs: { type: "button", role: "menuitem" },
      on: { click: function () { closeMenus(); fn(); } }
    }, [icon(cp), el("span", { text: label })]);
  }

  function closeMenus() {
    document.querySelectorAll(".menu").forEach(function (m) { m.remove(); });
    document.querySelectorAll(".chat-menu[aria-expanded]").forEach(function (b) {
      b.setAttribute("aria-expanded", "false");
    });
    state.menuFor = null;
  }

  async function renameChat(c) {
    var next = window.prompt("Rename chat", c.title || "");
    if (next == null) return;
    next = next.trim();
    if (!next || next === c.title) return;
    try {
      await api("POST", "/api/chats/" + c.id + "/rename", { title: next });
    } catch (_) { return; }
    refreshState();
    if (c.id === state.openId) refreshChat();
  }

  async function pinChat(c) {
    try {
      await api("POST", "/api/chats/" + c.id + "/pin", { pinned: !c.pinned });
    } catch (_) { return; }
    refreshState();
  }

  async function deleteChat(c) {
    if (!window.confirm('Delete "' + (c.title || "Untitled chat") + '"? This cannot be undone.')) return;
    try {
      await api("DELETE", "/api/chats/" + c.id);
    } catch (_) { return; }
    if (c.id === state.openId) openChat(null);
    refreshState();
  }

  // Provider and model selects. Provider change POSTs that provider's current model.
  function renderPicker() {
    if (!state.data) return;
    var providers = state.data.providers || [];
    var sig = JSON.stringify([providers, state.data.active_provider]);
    if (sig === state.pickerSig) return;
    state.pickerSig = sig;
    dom.providerSelect.replaceChildren.apply(dom.providerSelect, providers.map(function (p, i) {
      return el("option", { text: p.name, attrs: { value: String(i) } });
    }));
    dom.providerSelect.value = String(state.data.active_provider);
    dom.providerSelect.disabled = !providers.length;
    renderModels();
  }

  function renderModels() {
    var providers = (state.data && state.data.providers) || [];
    var p = providers[Number(dom.providerSelect.value)];
    var models = p ? p.models.slice() : [];
    if (p && p.model && models.indexOf(p.model) === -1) models.unshift(p.model);
    dom.modelSelect.replaceChildren.apply(dom.modelSelect, models.map(function (m) {
      return el("option", { text: m, attrs: { value: m } });
    }));
    dom.modelSelect.value = p && p.model ? p.model : "";
    dom.modelSelect.disabled = !models.length;
  }

  async function setModel(provider, model) {
    try {
      await api("POST", "/api/model", { provider: provider, model: model });
    } catch (_) { /* shown by api() */ }
    refreshState();
  }

  // ---- Events (SSE) ------------------------------------------------------------

  // The server sends "hello" as soon as a stream opens. Without it in this long, something on
  // the way (a Cloudflare quick tunnel buffers event streams) holds the stream back: poll.
  var HELLO_WAIT_MS = 5000;
  var POLL_RUNNING_MS = 1000;
  var POLL_IDLE_MS = 3000;
  var POLL_HIDDEN_MS = 15000;

  function connectStream() {
    if (state.es || state.pollTimer || !state.loggedIn) return;
    // Quick tunnels never pass event streams through, so don't hold a connection open.
    if (/\.trycloudflare\.com$/i.test(location.hostname)) { startPolling(); return; }
    var es = new EventSource("/api/events");
    state.es = es;
    // Counted from the request, not from "open": a buffering proxy may hold back even the
    // response headers, and then "open" never comes.
    clearTimeout(state.helloTimer);
    state.helloTimer = setTimeout(function () {
      if (state.es !== es) return;
      es.close();
      state.es = null;
      startPolling();
    }, HELLO_WAIT_MS);
    es.onopen = function () {
      state.retryDelay = 1000;
      refreshState();
      refreshChat();
    };
    es.onmessage = function (ev) {
      var msg;
      try { msg = JSON.parse(ev.data); } catch (_) { return; }
      if (msg.type === "hello") { clearTimeout(state.helloTimer); return; }
      // "list" also means the browser fell behind, so the open chat is refetched as well.
      if (msg.type === "list") { refreshState(); refreshChat(); }
      else if (msg.type === "chat" && msg.id === state.openId) refreshChat();
    };
    es.onerror = function () {
      // The browser retries by itself while CONNECTING. Only a CLOSED stream needs us.
      if (es.readyState !== EventSource.CLOSED) return;
      es.close();
      if (state.es === es) state.es = null;
      refreshState(); // a 401 here sends the user back to the login form
      scheduleReconnect();
    };
  }

  function scheduleReconnect() {
    if (!state.loggedIn || state.retryTimer) return;
    var wait = state.retryDelay;
    state.retryDelay = Math.min(wait * 2, 30000);
    state.retryTimer = setTimeout(function () {
      state.retryTimer = null;
      connectStream();
    }, wait);
  }

  // Asks for the state and the open chat on a timer: often while a reply is written, rarely
  // while the page is hidden.
  function startPolling() {
    if (state.pollTimer || !state.loggedIn) return;
    var tick = function () {
      refreshState();
      refreshChat();
      var wait = document.hidden ? POLL_HIDDEN_MS
        : (state.chat && state.chat.running) ? POLL_RUNNING_MS : POLL_IDLE_MS;
      state.pollTimer = setTimeout(tick, wait);
    };
    state.pollTimer = setTimeout(tick, POLL_RUNNING_MS);
  }

  function closeStream() {
    if (state.es) state.es.close();
    state.es = null;
    clearTimeout(state.helloTimer);
    clearTimeout(state.pollTimer);
    state.pollTimer = null;
    clearTimeout(state.retryTimer);
    state.retryTimer = null;
    state.retryDelay = 1000;
  }

  // ---- Chat view --------------------------------------------------------------

  function openChat(id, opts) {
    opts = opts || {};
    if (id && id === state.openId && state.chat) {
      closeSidebarOnPhone();
      return;
    }
    state.openId = id;
    state.chat = null;
    mdCache.clear();
    if (state.optimistic && state.optimistic.chat !== id) state.optimistic = null;
    setHash(id, opts.replace);
    closeMenus();
    renderChat();
    renderSidebar();
    if (id) {
      clearTimeout(chatThrottle.timer);
      chatThrottle.timer = null;
      fetchChatNow();
    }
    closeSidebarOnPhone();
    if (!isPhone()) dom.input.focus();
  }

  // Asks for the open chat, at most once per CHAT_THROTTLE_MS. Calls inside the window
  // are answered by one trailing fetch, so the final state is always requested.
  function refreshChat() {
    if (!state.openId) return;
    var wait = chatThrottle.at + CHAT_THROTTLE_MS - Date.now();
    if (wait <= 0) {
      fetchChatNow();
    } else if (!chatThrottle.timer) {
      chatThrottle.timer = setTimeout(function () {
        chatThrottle.timer = null;
        refreshChat();
      }, wait);
    }
  }

  // Fetches the open chat. Responses for a chat that is no longer open are dropped.
  var fetchChatNow = coalesce(async function () {
    var id = state.openId;
    if (!id) return;
    chatThrottle.at = Date.now();
    var chat = await api("GET", "/api/chats/" + id);
    if (state.openId !== id) return;
    var opt = state.optimistic;
    if (opt && opt.chat === id && visibleCount(chat.messages) > opt.base) state.optimistic = null;
    state.chat = chat;
    renderChat();
  });

  // Set of details keys (message index + call id) that are open in the DOM now.
  function openDetailKeys() {
    var keys = new Set();
    dom.messages.querySelectorAll("details[data-key][open]").forEach(function (d) {
      keys.add(d.dataset.key);
    });
    return keys;
  }

  function renderChat() {
    var id = state.openId;
    var chat = state.chat;
    var sameChat = state.renderedId === id;
    var atBottom = dom.messages.scrollHeight - dom.messages.scrollTop - dom.messages.clientHeight < 80;
    state.restoreKeys = sameChat ? openDetailKeys() : new Set();
    state.renderedId = id;

    var running = !!(chat && chat.running);
    dom.title.textContent = chat ? (chat.title || "Untitled chat") : (id ? "" : "New chat");

    var nodes = buildMessages(chat, id);
    dom.greeting.textContent = greetingText();
    dom.greeting.hidden = nodes.length > 0 || (!!id && !chat);
    dom.messages.replaceChildren.apply(dom.messages, [dom.greeting].concat(nodes));

    dom.stop.hidden = !running;
    dom.send.hidden = running;

    if (!sameChat || atBottom) dom.messages.scrollTop = dom.messages.scrollHeight;
    state.restoreKeys = new Set();

    state.chatError = chat && chat.error ? chat.error : null;
    paintBanner();
  }

  function buildMessages(chat, id) {
    var nodes = [];
    var used = new Set(); // approval indexes already shown under a tool call
    var optimisticShown = !!(state.optimistic && state.optimistic.chat === id);
    if (chat) {
      var msgs = chat.messages || [];
      var skip = new Set(); // tool results folded into their assistant message
      var lastAssistant = -1;
      msgs.forEach(function (m, i) { if (m.role === "assistant") lastAssistant = i; });
      msgs.forEach(function (m, i) {
        if (skip.has(i)) return;
        if (m.role === "user") {
          nodes.push(userNode(m));
        } else if (m.role === "assistant") {
          var canRegen = i === lastAssistant && !chat.running && !optimisticShown;
          nodes.push(assistantNode(msgs, i, chat, skip, used, canRegen));
        }
        // "system" and orphan "tool" messages are not shown.
      });
      var orphans = (chat.approvals || []).filter(function (a, k) { return !used.has(k); });
      if (orphans.length) {
        nodes.push(el("div", { cls: "msg assistant" }, orphans.map(function (a) {
          return approvalNode(a, chat.id);
        })));
      }
    }
    if (optimisticShown) {
      nodes.push(userNode({ content: state.optimistic.text, attachments: [], cancelled: false }));
    }
    if (chat && chat.running) {
      nodes.push(el("div", { cls: "msg assistant" }, [
        el("span", { cls: "typing" }, [el("i"), el("i"), el("i")])
      ]));
    }
    return nodes;
  }

  function userNode(m) {
    var kids = [];
    if (m.attachments && m.attachments.length) {
      kids.push(el("div", { cls: "attachments" }, m.attachments.map(function (a) {
        return el("span", { cls: "chip", text: a.name });
      })));
    }
    kids.push(m.content || "");
    return el("div", { cls: "msg user" + (m.cancelled ? " cancelled" : "") }, kids);
  }

  // Renders one assistant message. Tool results are the tool messages that directly follow it.
  function assistantNode(msgs, i, chat, skip, used, canRegen) {
    var m = msgs[i];
    var kids = [];
    if (m.reasoning) kids.push(reasoningNode(m.reasoning, i));
    if (m.content) {
      var body = el("div");
      body.innerHTML = renderMarkdownCached(chat.id, i, m.role, m.content);
      kids.push(body);
    }
    var results = [];
    for (var j = i + 1; j < msgs.length && msgs[j].role === "tool"; j++) {
      results.push(msgs[j]);
      skip.add(j);
    }
    (m.tool_calls || []).forEach(function (call) {
      var mine = results.filter(function (r) { return r.tool_call_id === call.id; });
      kids.push(toolCard(call, mine, i, chat));
      if (!mine.length) {
        var k = (chat.approvals || []).findIndex(function (a, idx) {
          return a.call_id === call.id && !used.has(idx);
        });
        if (k !== -1) {
          used.add(k);
          kids.push(approvalNode(chat.approvals[k], chat.id));
        }
      }
    });
    if (m.stats) kids.push(el("p", { cls: "msg-stats", text: m.stats }));
    if (canRegen) {
      kids.push(el("div", { cls: "msg-actions" }, [
        el("button", { attrs: { type: "button" }, on: { click: regenerate } }, [
          icon(ICON.REGEN), el("span", { text: "Regenerate" })
        ])
      ]));
    }
    return el("div", { cls: "msg assistant" + (m.cancelled ? " cancelled" : "") }, kids);
  }

  // CleanMarkdown output for one message, reused while its content is unchanged.
  function renderMarkdownCached(chatId, index, role, content) {
    var key = chatId + "\u0000" + index + "\u0000" + role;
    var hit = mdCache.get(key);
    if (hit && hit.content === content) return hit.html;
    var html = CleanMarkdown.render(content);
    if (mdCache.size >= MD_CACHE_MAX) mdCache.clear();
    mdCache.set(key, { content: content, html: html });
    return html;
  }

  function makeDetails(cls, key) {
    var d = el("details", { cls: cls, data: { key: key } });
    d.open = state.restoreKeys.has(key);
    return d;
  }

  function reasoningNode(text, i) {
    var d = makeDetails("reasoning", "m" + i + ":reasoning");
    d.appendChild(el("summary", { text: "Reasoning" }));
    d.appendChild(el("div", { text: text }));
    return d;
  }

  function toolCard(call, results, i, chat) {
    var failed = results.some(function (r) { return r.is_error; });
    var approvals = chat.approvals || [];
    var status;
    if (results.length) status = failed ? "Failed" : "Done";
    else if (chat.running_tool === call.id) status = "Running…";
    else if (approvals.some(function (a) { return a.call_id === call.id; })) status = "Waiting for approval";
    else status = "Queued";

    var sandboxed = results.find(function (r) { return r.sandbox; });
    var sandboxImage = sandboxed ? sandboxed.sandbox : null;

    var d = makeDetails("tool-card" + (failed ? " error" : ""), "m" + i + ":" + call.id);
    d.appendChild(el("summary", null, [
      icon(ICON.TOOL),
      el("span", { cls: "tool-name", text: call.label || call.name }),
      el("span", { cls: "tool-status", text: status }),
      sandboxImage ? el("span", { cls: "tool-sandbox", text: "in " + sandboxImage }) : null
    ]));
    d.appendChild(el("pre", { cls: "tool-args", text: prettyJson(call.arguments) }));
    if (results.length) {
      d.appendChild(el("pre", {
        cls: "tool-result",
        text: results.map(function (r) { return r.content || ""; }).join("\n")
      }));
    }
    return d;
  }

  function approvalNode(a, chatId) {
    var card = el("div", { cls: "approval" });
    var answers = [["Once", "once", "btn-primary"]];
    if (a.can_always_tool) answers.push(["Always allow this tool", "always_tool", "btn"]);
    if (a.can_always_server) answers.push(["Always allow this server", "always_server", "btn"]);
    answers.push(["Deny", "deny", "btn-danger"]);
    var buttons = answers.map(function (ans) {
      return el("button", {
        cls: ans[2], text: ans[0], attrs: { type: "button" },
        on: { click: function () { answerApproval(chatId, a.call_id, ans[1], card); } }
      });
    });
    card.appendChild(el("div", { cls: "approval-title", text: "Approval needed" }));
    var shown = a.label || a.name;
    card.appendChild(el("div", { cls: "muted", text: a.server_name ? shown + " on " + a.server_name : shown }));
    card.appendChild(el("pre", { text: prettyJson(a.arguments) }));
    card.appendChild(el("div", { cls: "approval-actions" }, buttons));
    return card;
  }

  function setButtonsDisabled(card, disabled) {
    card.querySelectorAll("button").forEach(function (b) { b.disabled = disabled; });
  }

  async function answerApproval(chatId, callId, answer, card) {
    setButtonsDisabled(card, true);
    try {
      await api("POST", "/api/chats/" + chatId + "/approvals/" + encodeURIComponent(callId), { answer: answer });
    } catch (_) {
      setButtonsDisabled(card, false);
      return;
    }
    refreshChat();
    refreshState();
  }

  async function regenerate() {
    var id = state.openId;
    if (!id) return;
    try {
      await api("POST", "/api/chats/" + id + "/regenerate");
    } catch (_) { return; }
    refreshChat();
    refreshState();
  }

  async function stopReply() {
    var id = state.openId;
    if (!id || !state.chat || !state.chat.running) return;
    try {
      await api("POST", "/api/chats/" + id + "/stop");
    } catch (_) { /* shown by api() */ }
    refreshChat();
    refreshState();
  }

  // ---- Composer ---------------------------------------------------------------

  function autosize() {
    dom.input.style.height = "auto";
    dom.input.style.height = dom.input.scrollHeight + "px";
  }

  // Sends the composer text. The message shows at once and is dropped if the send fails.
  async function send() {
    var text = dom.input.value.trim();
    if (!text || state.sending) return;
    state.sending = true;
    var id = state.openId;
    var base = state.chat ? visibleCount(state.chat.messages) : 0;
    state.optimistic = { chat: id, text: text, base: base };
    dom.input.value = "";
    autosize();
    renderChat();
    try {
      var res = await api("POST", "/api/chats/" + (id || "new") + "/messages", { text: text }, true);
      if (state.optimistic && state.optimistic.chat === id) state.optimistic.chat = res.id;
      if (state.openId === id) {
        state.openId = res.id;
        setHash(res.id, true);
      }
      renderChat();
      refreshChat();
      refreshState();
      if (!isPhone()) dom.input.focus();
    } catch (err) {
      if (state.optimistic && state.optimistic.chat === id && state.optimistic.text === text) {
        state.optimistic = null;
      }
      if (!dom.input.value) {
        dom.input.value = text;
        autosize();
      }
      renderChat();
      var status = err && err.status;
      flash(status === 409 ? "A reply is still being written" : (err && err.error) || "Could not send");
    } finally {
      state.sending = false;
    }
  }

  // ---- Copy ---------------------------------------------------------------------

  function copyCode(codeEl, btn) {
    var done = function () {
      btn.textContent = "Copied";
      setTimeout(function () { btn.textContent = "Copy"; }, 1500);
    };
    var fallback = function () {
      var range = document.createRange();
      range.selectNodeContents(codeEl);
      var sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(range);
      var ok = false;
      try { ok = document.execCommand("copy"); } catch (_) { ok = false; }
      sel.removeAllRanges();
      if (ok) done();
      else flash("Could not copy the code");
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(codeEl.textContent).then(done, fallback);
    } else {
      fallback();
    }
  }

  // ---- Command palette ----------------------------------------------------------

  function paletteCommands() {
    var cmds = [{ label: "New chat", hint: MAC ? "Cmd+Shift+O" : "Ctrl+Shift+O", run: newChat }];
    ((state.data && state.data.chats) || []).forEach(function (c) {
      cmds.push({ label: c.title || "Untitled chat", hint: "Chat", run: function () { openChat(c.id); } });
    });
    cmds.push({ label: "Toggle sidebar", run: toggleSidebar });
    cmds.push({ label: "Log out", run: logout });
    return cmds;
  }

  function openPalette() {
    state.palFocus = document.activeElement;
    dom.palette.hidden = false;
    dom.paletteInput.value = "";
    state.palIndex = 0;
    renderPalette();
    dom.paletteInput.focus();
  }

  function closePalette() {
    if (dom.palette.hidden) return;
    dom.palette.hidden = true;
    var back = state.palFocus;
    state.palFocus = null;
    if (back && document.contains(back) && typeof back.focus === "function") back.focus();
  }

  function renderPalette() {
    var q = dom.paletteInput.value.trim().toLowerCase();
    state.palItems = paletteCommands().filter(function (c) {
      return !q || c.label.toLowerCase().indexOf(q) !== -1;
    });
    if (state.palIndex >= state.palItems.length) state.palIndex = 0;
    var items = state.palItems.map(function (c, i) {
      var kids = [el("span", { text: c.label })];
      if (c.hint) kids.push(el("span", { cls: "palette-hint", text: c.hint }));
      return el("li", { attrs: { role: "presentation" } }, [
        el("button", {
          attrs: { type: "button", role: "option", "aria-selected": String(i === state.palIndex) },
          on: { click: function () { runPalette(i); } }
        }, kids)
      ]);
    });
    if (!items.length) items.push(el("li", { attrs: { role: "presentation" }, text: "No matches" }));
    dom.paletteList.replaceChildren.apply(dom.paletteList, items);
  }

  function movePalette(delta) {
    var n = state.palItems.length;
    if (!n) return;
    state.palIndex = (state.palIndex + delta + n) % n;
    var buttons = dom.paletteList.querySelectorAll('button[role="option"]');
    buttons.forEach(function (b, i) { b.setAttribute("aria-selected", String(i === state.palIndex)); });
    if (buttons[state.palIndex]) buttons[state.palIndex].scrollIntoView({ block: "nearest" });
  }

  function runPalette(i) {
    var cmd = state.palItems[i];
    closePalette();
    if (cmd) cmd.run();
  }

  // ---- Actions shared by buttons, menus and shortcuts -------------------------

  async function newChat() {
    closeMenus();
    var res;
    try {
      res = await api("POST", "/api/chats");
    } catch (_) { return; }
    openChat(res.id);
    refreshState();
  }

  async function logout() {
    try {
      await api("POST", "/api/logout");
    } catch (_) { /* log out locally anyway */ }
    showLogin();
  }

  // ---- Event wiring -----------------------------------------------------------

  dom.loginForm.addEventListener("submit", async function (e) {
    e.preventDefault();
    dom.loginError.textContent = "";
    dom.loginSubmit.disabled = true;
    try {
      await api("POST", "/api/login", { password: dom.loginPassword.value }, true);
    } catch (err) {
      dom.loginSubmit.disabled = false;
      dom.loginError.textContent = err.status === 401 ? "Wrong password"
        : err.status === 429 ? "Too many attempts. Try again in a minute."
        : (err.error || "Could not log in");
      dom.loginPassword.select();
      return;
    }
    dom.loginSubmit.disabled = false;
    enterApp();
  });

  dom.logout.addEventListener("click", logout);
  dom.newChat.addEventListener("click", newChat);
  dom.search.addEventListener("input", renderSidebar);
  dom.sidebarToggle.addEventListener("click", toggleSidebar);
  dom.backdrop.addEventListener("click", function () {
    dom.app.classList.remove("sidebar-open");
    syncSidebarToggle();
  });

  dom.providerSelect.addEventListener("change", function () {
    var providers = (state.data && state.data.providers) || [];
    var idx = Number(dom.providerSelect.value);
    var p = providers[idx];
    if (!p) return;
    renderModels();
    setModel(idx, p.model);
  });
  dom.modelSelect.addEventListener("change", function () {
    setModel(Number(dom.providerSelect.value), dom.modelSelect.value);
  });

  dom.form.addEventListener("submit", function (e) {
    e.preventDefault();
    send();
  });
  dom.input.addEventListener("input", autosize);
  dom.input.addEventListener("keydown", function (e) {
    if (e.key !== "Enter" || e.isComposing) return;
    // Ctrl/Cmd+Enter always sends. Enter sends only when send_with_enter is on (Shift+Enter then adds a line).
    if (e.ctrlKey || e.metaKey || (sendsOnEnter() && !e.shiftKey)) {
      e.preventDefault();
      send();
    }
  });
  dom.stop.addEventListener("click", stopReply);

  dom.messages.addEventListener("click", function (e) {
    var btn = e.target.closest ? e.target.closest(".copy-btn") : null;
    if (!btn) return;
    var block = btn.closest(".code-block");
    var code = block && (block.querySelector("pre code") || block.querySelector("pre"));
    if (code) copyCode(code, btn);
  });

  dom.paletteInput.addEventListener("input", function () {
    state.palIndex = 0;
    renderPalette();
  });
  dom.paletteInput.addEventListener("keydown", function (e) {
    if (e.key === "ArrowDown") { e.preventDefault(); movePalette(1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); movePalette(-1); }
    else if (e.key === "Enter" && !e.isComposing) { e.preventDefault(); runPalette(state.palIndex); }
  });

  // Click outside an open menu or palette closes it.
  document.addEventListener("mousedown", function (e) {
    if (state.menuFor && !e.target.closest(".menu, .chat-menu")) closeMenus();
    if (!dom.palette.hidden && !dom.palette.contains(e.target)) closePalette();
  });

  // Keyboard shortcuts (docs/reference/keyboard-shortcuts.md).
  // Ctrl/Cmd+N is reserved by browsers, so "new chat" is Ctrl/Cmd+Shift+O, as in other web chat apps.
  // Ctrl/Cmd+, (settings) has no browser counterpart and is not implemented.
  document.addEventListener("keydown", function (e) {
    if (!state.loggedIn) return;
    var mod = e.ctrlKey || e.metaKey;
    var key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    if (mod && !e.altKey && key === "k") {
      e.preventDefault();
      if (dom.palette.hidden) openPalette();
      else closePalette();
      return;
    }
    if (mod && !e.altKey && e.shiftKey && key === "o") {
      e.preventDefault();
      newChat();
      return;
    }
    if (mod && !e.altKey && !e.shiftKey && key === "b") {
      e.preventDefault();
      toggleSidebar();
      return;
    }
    if (key === "Escape") {
      if (!dom.palette.hidden) closePalette();
      else if (state.menuFor) closeMenus();
      else if (state.chat && state.chat.running) stopReply();
      return;
    }
    if (key === "/" && !mod && !e.altKey && !isEditable(e.target)) {
      e.preventDefault();
      dom.input.focus();
    }
  });

  window.addEventListener("hashchange", function () {
    if (!state.loggedIn) return;
    var id = parseHashId();
    if (id !== state.openId) openChat(id, { replace: true });
  });

  listen(DARK, function () {
    if (!state.data || state.data.theme === "system") applyTheme();
  });
  listen(PHONE, function () {
    dom.app.classList.remove("sidebar-open", "sidebar-collapsed");
    syncSidebarToggle();
  });

  // ---- Boot -------------------------------------------------------------------

  async function boot() {
    var session;
    try {
      session = await api("GET", "/api/session", undefined, true);
    } catch (err) {
      showLogin();
      dom.loginError.textContent = err.error || "Could not reach the server";
      return;
    }
    state.plainHttp = !!session.plain_http;
    if (session.logged_in) enterApp();
    else showLogin();
  }

  boot();
})();

"use strict";

(function () {
  var csrf = (document.querySelector('meta[name="cc-csrf"]') || {}).content || "";
  var flash = document.getElementById("flash");
  var baseTitle = document.title.replace(/^\(\d+\) /, "");

  function say(text, bad) {
    if (!flash) return;
    flash.textContent = text;
    flash.className = bad ? "bad" : "";
  }

  function syncTitle() {
    var region = document.querySelector("[data-waiting]");
    var waiting = region ? parseInt(region.getAttribute("data-waiting"), 10) : 0;
    document.title = waiting > 0 ? "(" + waiting + ") " + baseTitle : baseTitle;
  }

  document.addEventListener("htmx:configRequest", function (event) {
    event.detail.headers["X-CC-Token"] = csrf;
  });

  var asker = null;

  function closeConfirm(panel) {
    var slot = panel.parentElement;
    if (slot && slot.id === "confirm-slot") {
      slot.textContent = "";
      if (asker && document.body.contains(asker)) asker.focus();
      asker = null;
      return true;
    }
    return false;
  }

  function openConfirm() {
    var yes = document.querySelector("[data-confirm] [data-confirm-yes]");
    if (yes) yes.focus();
  }

  document.addEventListener("htmx:beforeRequest", function (event) {
    if (event.detail.requestConfig.verb === "get" && event.detail.elt.getAttribute("hx-target") === "#confirm-slot") {
      asker = event.detail.elt;
    }
  });

  document.addEventListener("htmx:afterSwap", function (event) {
    if (event.detail.target && event.detail.target.id === "confirm-slot") openConfirm();
  });

  document.addEventListener("click", function (event) {
    var cancel = event.target.closest && event.target.closest("[data-confirm-cancel]");
    if (!cancel) return;
    var panel = cancel.closest("[data-confirm]");
    if (panel && closeConfirm(panel)) event.preventDefault();
  });

  document.addEventListener("keydown", function (event) {
    if (event.key !== "Escape") return;
    var panel = document.querySelector("[data-confirm]");
    if (panel && closeConfirm(panel)) event.preventDefault();
  });

  document.addEventListener("htmx:afterRequest", function (event) {
    var xhr = event.detail.xhr;
    if (!xhr) return;
    var body = null;
    try {
      body = JSON.parse(xhr.responseText);
    } catch (ignored) {
      body = null;
    }
    if (event.detail.requestConfig.verb === "get") {
      if (!event.detail.successful && event.detail.elt.getAttribute("hx-target") === "#confirm-slot") {
        say((body && body.error) || "request failed (" + xhr.status + ")", true);
      }
      return;
    }
    if (event.detail.successful) {
      say((body && body.message) || "done", false);
      var form = event.detail.elt;
      if (form && form.tagName === "FORM" && form.hasAttribute("data-reset")) form.reset();
      if (form && form.hasAttribute("data-move-form")) {
        showMove(form, false);
        form.reset();
      }
      if (form && form.hasAttribute("data-confirm-form")) {
        var panel = form.closest("[data-confirm]");
        var cancel = form.querySelector("[data-confirm-cancel]");
        if (panel && !closeConfirm(panel) && cancel) window.location.href = cancel.getAttribute("href");
      }
    } else {
      movedRow = null;
      say((body && body.error) || "request failed (" + xhr.status + ")", true);
    }
  });

  // Project nodes: open unless the viewer closed them; kept per viewer only.
  var NODES_KEY = "cc-nodes";
  var closedNodes = {};
  try {
    closedNodes = JSON.parse(window.localStorage.getItem(NODES_KEY) || "{}") || {};
  } catch (ignored) {
    closedNodes = {};
  }

  function saveNodes() {
    try {
      window.localStorage.setItem(NODES_KEY, JSON.stringify(closedNodes));
    } catch (ignored) {
      /* private window or blocked storage: the choice lasts until reload */
    }
  }

  function applyNodes() {
    Array.prototype.forEach.call(document.querySelectorAll("details[data-node]"), function (node) {
      var want = closedNodes[node.id] !== true;
      if (node.open !== want) node.open = want;
      var summary = node.querySelector("summary");
      if (summary) summary.setAttribute("aria-expanded", String(node.open));
    });
  }

  // The click is the viewer's own choice; the toggle event also fires when a
  // refresh swaps in fresh markup, which must not overwrite that choice.
  document.addEventListener("click", function (event) {
    var summary = event.target.closest && event.target.closest("details[data-node] > summary");
    if (!summary) return;
    var node = summary.parentElement;
    setTimeout(function () {
      if (node.open) {
        delete closedNodes[node.id];
      } else {
        closedNodes[node.id] = true;
      }
      summary.setAttribute("aria-expanded", String(node.open));
      saveNodes();
    }, 0);
  });

  // The Move panel: opened by the row's button, an inline form, no modal.
  function moveParts(form) {
    return {
      select: form.querySelector("[data-move-to]"),
      wrap: form.querySelector("[data-move-new-wrap]"),
      input: form.querySelector("[data-move-new]"),
      go: form.querySelector("[data-move-go]"),
      toggle: document.querySelector('[aria-controls="' + form.id + '"]'),
    };
  }

  function syncMove(form) {
    var parts = moveParts(form);
    var choosing = parts.select.options[parts.select.selectedIndex];
    var isNew = parts.select.value === "";
    parts.wrap.hidden = !isNew;
    var ready = !!choosing && !choosing.disabled && (!isNew || parts.input.value.trim() !== "");
    parts.go.disabled = !ready;
  }

  function showMove(form, shown) {
    var parts = moveParts(form);
    form.hidden = !shown;
    if (parts.toggle) parts.toggle.setAttribute("aria-expanded", String(shown));
    if (shown) {
      syncMove(form);
    }
  }

  function closeMove(form) {
    var parts = moveParts(form);
    showMove(form, false);
    if (parts.toggle) parts.toggle.focus();
  }

  document.addEventListener("click", function (event) {
    var toggle = event.target.closest && event.target.closest("[data-move-toggle]");
    if (toggle) {
      var form = document.getElementById(toggle.getAttribute("aria-controls"));
      if (!form) return;
      if (form.hidden) {
        showMove(form, true);
        moveParts(form).select.focus();
      } else {
        closeMove(form);
      }
      return;
    }
    var cancel = event.target.closest && event.target.closest("[data-move-cancel]");
    if (cancel) closeMove(cancel.closest("[data-move-form]"));
  });

  document.addEventListener("change", function (event) {
    var form = event.target.closest && event.target.closest("[data-move-form]");
    if (!form) return;
    syncMove(form);
    if (event.target.hasAttribute("data-move-to") && event.target.value === "") moveParts(form).input.focus();
  });

  document.addEventListener("input", function (event) {
    var form = event.target.closest && event.target.closest("[data-move-form]");
    if (form) syncMove(form);
  });

  document.addEventListener("keydown", function (event) {
    if (event.key !== "Escape") return;
    var active = document.activeElement;
    var form = active && active.closest && active.closest("[data-move-form]");
    if (form && !form.hidden) {
      closeMove(form);
      event.preventDefault();
    }
  });

  // "New project…" sends the typed name as the project; the endpoint creates it.
  var movedRow = null;
  document.addEventListener("htmx:configRequest", function (event) {
    var form = event.detail.elt;
    if (!form || !form.hasAttribute || !form.hasAttribute("data-move-form")) return;
    var parts = moveParts(form);
    if (parts.select.value === "") event.detail.parameters.project = parts.input.value.trim();
    movedRow = form.closest("li.row");
  });

  // A refresh swaps the whole region; keep what the viewer is in the middle
  // of: open panels, typed values, focus, and the folds of the menus.
  var kept = null;

  function keep() {
    var region = document.getElementById("sessions");
    if (!region) return null;
    var fields = {};
    var inUse = '[data-move-form]:not([hidden]) :is(input, select, textarea)[id], ' +
      "details.menu[open] :is(input, select, textarea)[id]";
    Array.prototype.forEach.call(region.querySelectorAll(inUse), function (field) {
      fields[field.id] = field.value;
    });
    var active = document.activeElement;
    return {
      fields: fields,
      panels: Array.prototype.map.call(region.querySelectorAll("[data-move-form]:not([hidden])"), function (f) {
        return f.id;
      }),
      menus: Array.prototype.map.call(region.querySelectorAll("details[open][id]:not([data-node])"), function (d) {
        return d.id;
      }),
      focus: active && active.id && region.contains(active) ? active.id : null,
    };
  }

  function restore(state) {
    state.menus.forEach(function (id) {
      var details = document.getElementById(id);
      if (details) details.open = true;
    });
    state.panels.forEach(function (id) {
      var form = document.getElementById(id);
      if (form) form.hidden = false;
    });
    Object.keys(state.fields).forEach(function (id) {
      var field = document.getElementById(id);
      if (!field || field.value === state.fields[id]) return;
      var known = field.tagName !== "SELECT" || Array.prototype.some.call(field.options, function (option) {
        return option.value === state.fields[id];
      });
      if (known) field.value = state.fields[id];
    });
    state.panels.forEach(function (id) {
      var form = document.getElementById(id);
      if (!form) return;
      var toggle = moveParts(form).toggle;
      if (toggle) toggle.setAttribute("aria-expanded", "true");
      syncMove(form);
    });
    var target = state.focus && document.getElementById(state.focus);
    if (target && target !== document.activeElement) {
      target.focus();
      if (typeof target.setSelectionRange === "function" && target.type === "text") {
        var end = target.value.length;
        target.setSelectionRange(end, end);
      }
    }
  }

  document.addEventListener("htmx:beforeSwap", function () {
    kept = keep();
  });
  document.addEventListener("htmx:afterSwap", function () {
    applyNodes();
    if (kept) restore(kept);
  });
  document.addEventListener("htmx:afterSettle", function () {
    kept = null;
    syncTitle();
    if (movedRow) {
      var row = document.getElementById(movedRow.id);
      var toggle = row && row.querySelector("[data-move-toggle]");
      if (toggle) toggle.focus();
      movedRow = null;
    }
  });

  applyNodes();

  syncTitle();

  if (window.EventSource) {
    var pending = null;
    var source = new EventSource("/events");
    source.onmessage = function () {
      if (pending) return;
      pending = setTimeout(function () {
        pending = null;
        document.body.dispatchEvent(new CustomEvent("cc-changed"));
      }, 50);
    };
  }
})();

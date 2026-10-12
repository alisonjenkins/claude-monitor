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
      if (form && form.hasAttribute("data-confirm-form")) {
        var panel = form.closest("[data-confirm]");
        var cancel = form.querySelector("[data-confirm-cancel]");
        if (panel && !closeConfirm(panel) && cancel) window.location.href = cancel.getAttribute("href");
      }
    } else {
      say((body && body.error) || "request failed (" + xhr.status + ")", true);
    }
  });

  var opened = [];
  document.addEventListener("htmx:beforeSwap", function () {
    opened = Array.prototype.map.call(document.querySelectorAll("details[open][id]"), function (d) {
      return d.id;
    });
  });
  document.addEventListener("htmx:afterSettle", function () {
    opened.forEach(function (id) {
      var details = document.getElementById(id);
      if (details) details.open = true;
    });
    syncTitle();
  });

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

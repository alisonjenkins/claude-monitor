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

  document.addEventListener("htmx:afterRequest", function (event) {
    var xhr = event.detail.xhr;
    if (!xhr || event.detail.requestConfig.verb === "get") return;
    var body = null;
    try {
      body = JSON.parse(xhr.responseText);
    } catch (ignored) {
      body = null;
    }
    if (event.detail.successful) {
      say((body && body.message) || "done", false);
      var form = event.detail.elt;
      if (form && form.tagName === "FORM" && form.hasAttribute("data-reset")) form.reset();
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

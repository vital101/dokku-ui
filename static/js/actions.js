// Progressive enhancement for app actions (see templates/apps/show.html and
// templates/apps/partials/processes.html):
//
//  * an action POST swaps a "run" fragment into the modal, which then streams
//    the dokku command's output over Server-Sent Events;
//  * every .app-action control is disabled while a run is in flight and the
//    clicked one shows a spinner (styles in assets/input.css);
//  * when the run finishes, a green/red banner states the outcome, the page's
//    data fragment refreshes, and buttons come back.
//
// Without this script the forms fall back to plain POST + redirect.
(function () {
  "use strict";

  var runSource = null;
  var pendingButton = null;

  function overlay() {
    return document.querySelector("[data-modal-overlay]");
  }

  function modalContent() {
    return document.getElementById("modal-content");
  }

  function openModal() {
    var o = overlay();
    if (o) o.setAttribute("data-open", "");
  }

  function closeModal() {
    var o = overlay();
    if (o) {
      o.removeAttribute("data-open");
      o.removeAttribute("data-run-open");
    }
    var c = modalContent();
    if (c) c.innerHTML = "";
  }

  function setBusy(button) {
    document.querySelectorAll(".app-action").forEach(function (el) {
      if (el.tagName === "A") {
        el.setAttribute("data-disabled", "");
      } else {
        el.disabled = true;
      }
    });
    if (button) button.setAttribute("data-busy", "");
  }

  function clearBusy() {
    document.querySelectorAll(".app-action").forEach(function (el) {
      el.removeAttribute("data-disabled");
      if (el.tagName !== "A") el.disabled = false;
    });
    document.querySelectorAll(".app-action[data-busy]").forEach(function (el) {
      el.removeAttribute("data-busy");
    });
    pendingButton = null;
  }

  function stopRun() {
    if (runSource) {
      runSource.close();
      runSource = null;
    }
  }

  function finishRun(run, ok, message) {
    var outcome = run.querySelector("[data-run-outcome]");
    if (outcome) {
      outcome.setAttribute("data-state", ok ? "ok" : "error");
      outcome.textContent = message;
      outcome.classList.remove("hidden");
    }
    var spinner = run.querySelector("[data-run-spinner]");
    if (spinner) spinner.classList.add("hidden");
    clearBusy();
  }

  function startRun(run) {
    stopRun();
    var url = run.getAttribute("data-run-url");
    if (!url) return;

    var log = run.querySelector("[data-run-log]");
    var finished = false;
    runSource = new EventSource(url);

    runSource.addEventListener("line", function (event) {
      if (!log) return;
      log.textContent += event.data + "\n";
      log.scrollTop = log.scrollHeight;
    });

    runSource.addEventListener("done", function (event) {
      finished = true;
      stopRun();
      var payload;
      try {
        payload = JSON.parse(event.data);
      } catch (err) {
        payload = { ok: false, message: "Unexpected server response." };
      }
      finishRun(run, !!payload.ok, payload.message || "");
      if (payload.redirect) {
        window.location.href = payload.redirect;
        return;
      }
      var refresh = run.getAttribute("data-refresh");
      if (refresh && window.htmx) {
        window.htmx.ajax("GET", refresh, { target: "#app-panel", swap: "innerHTML" });
      }
    });

    runSource.onerror = function () {
      // A reconnecting EventSource stays CONNECTING; only a killed stream
      // (expired run, server error) is CLOSED.
      if (finished || !runSource || runSource.readyState !== EventSource.CLOSED) return;
      stopRun();
      finishRun(run, false, "Lost connection to the action stream.");
    };
  }

  // Native submit (captured before htmx's own handler): fires for every
  // data-action-form submission — button click or Enter key — regardless of
  // how htmx shapes its event details.
  document.addEventListener(
    "submit",
    function (event) {
      var form = event.target;
      if (form && form.matches && form.matches("[data-action-form]")) {
        setBusy(pendingButton);
      }
    },
    true
  );

  document.body.addEventListener("htmx:afterSwap", function (event) {
    var target = event.detail.target;
    if (!target || target.id !== "modal-content") return;
    var o = overlay();
    var run = target.querySelector("[data-run]");
    if (o) {
      if (run) o.setAttribute("data-run-open", "");
      else o.removeAttribute("data-run-open");
    }
    openModal();
    if (run) {
      startRun(run);
    } else {
      // Validation errors and the delete-confirm dialog have no run; make
      // sure a failed submit never leaves the action buttons disabled.
      clearBusy();
    }
  });

  document.body.addEventListener("htmx:responseError", clearBusy);
  document.body.addEventListener("htmx:sendError", clearBusy);

  document.addEventListener("click", function (event) {
    var action = event.target.closest(".app-action");
    if (
      action &&
      action.tagName === "BUTTON" &&
      action.form &&
      action.form.matches("[data-action-form]")
    ) {
      pendingButton = action;
    }
    if (event.target.closest("[data-run-close]") || event.target.closest("[data-modal-close]")) {
      closeModal();
      return;
    }
    if (event.target.matches("[data-modal-overlay]")) {
      closeModal();
    }
  });

  document.addEventListener("keydown", function (event) {
    var o = overlay();
    if (event.key === "Escape" && o && o.hasAttribute("data-open")) {
      closeModal();
    }
  });

  // Copy-to-clipboard for readonly fields (push URL, deploy key). The field is
  // named by id in data-copy; the button briefly confirms.
  document.addEventListener("click", function (event) {
    var button = event.target.closest("[data-copy]");
    if (!button) return;
    var field = document.getElementById(button.getAttribute("data-copy"));
    if (!field || !navigator.clipboard) return;
    var value = field.value !== undefined ? field.value : field.textContent;
    navigator.clipboard.writeText(value).then(function () {
      var label = button.getAttribute("data-copy-label") || "Copy";
      button.textContent = "Copied";
      setTimeout(function () {
        button.textContent = label;
      }, 1500);
    });
  });
})();

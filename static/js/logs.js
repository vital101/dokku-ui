// Progressive enhancement for the live log panel (templates/apps/logs.html):
// a source selector (app / nginx access / nginx error), a Pause/Resume toggle
// and Clear, backed by the generic SSE stream at
// `/apps/{name}/logs/stream?source=…&tail=1`. Without JS the panel stays
// hidden and the bounded htmx log viewer is all that renders.
(function () {
  "use strict";

  var panel = document.getElementById("live-log-panel");
  if (!panel) return;

  var app = panel.getAttribute("data-app");
  // Service panels set an explicit stream URL; app panels build source-aware
  // URLs from the data-app attribute.
  var streamBase = panel.getAttribute("data-stream-base");
  var log = panel.querySelector("[data-live-log]");
  var toggle = panel.querySelector("[data-log-toggle]");
  var processInput = panel.querySelector("[data-log-process]");
  var source = null;
  var currentSource = "logs";

  function processFilter() {
    return processInput ? processInput.value.trim() : "";
  }

  function streamUrl() {
    if (streamBase) {
      return streamBase + "?tail=1";
    }
    var url =
      "/apps/" + app + "/logs/stream?source=" + currentSource + "&tail=1";
    var process = processFilter();
    // The process filter only applies to the app's own log source.
    if (process && currentSource === "logs") {
      url += "&process=" + encodeURIComponent(process);
    }
    return url;
  }

  function openStream() {
    stopStream();
    source = new EventSource(streamUrl());
    source.addEventListener("line", function (event) {
      log.textContent += event.data + "\n";
      log.scrollTop = log.scrollHeight;
    });
    source.addEventListener("error", function (event) {
      // A server-sent error (e.g. an unsupported source) is terminal — stop
      // instead of letting EventSource reconnect forever.
      stopStream();
      log.textContent += "Error: " + (event.data || "stream is not available") + "\n";
      log.scrollTop = log.scrollHeight;
    });
    source.onerror = function () {
      if (!source || source.readyState !== EventSource.CLOSED) return;
      stopStream();
      log.textContent += "Live stream ended.\n";
      log.scrollTop = log.scrollHeight;
    };
  }

  function stopStream() {
    if (source) {
      source.close();
      source = null;
    }
  }

  panel.querySelectorAll("[data-source]").forEach(function (button) {
    button.addEventListener("click", function () {
      currentSource = button.getAttribute("data-source");
      log.textContent = "";
      openStream();
    });
  });

  toggle.addEventListener("click", function () {
    if (source) {
      stopStream();
      toggle.textContent = "Resume";
      toggle.classList.remove("bg-emerald-600");
      toggle.classList.add("bg-slate-800");
    } else {
      openStream();
      toggle.textContent = "Pause";
      toggle.classList.add("bg-emerald-600");
      toggle.classList.remove("bg-slate-800");
    }
  });

  panel.querySelector("[data-log-clear]").addEventListener("click", function () {
    log.textContent = "";
  });

  if (processInput) {
    processInput.addEventListener("change", function () {
      if (!source) return;
      log.textContent = "";
      openStream();
    });
  }

  panel.classList.remove("hidden");
  openStream();
})();
(function () {
  var overlay = document.querySelector("[data-palette-overlay]");
  if (!overlay) return;
  var input = overlay.querySelector("[data-palette-input]");
  var results = overlay.querySelector("[data-palette-results]");
  var template = document.querySelector("[data-palette-item-template]");
  var items = null;
  var filtered = [];
  var active = 0;

  function load() {
    if (items) return Promise.resolve(items);
    return fetch("/palette.json")
      .then(function (response) {
        return response.json();
      })
      .then(function (data) {
        items = data.items;
        return items;
      });
  }

  function render() {
    results.innerHTML = "";
    filtered.forEach(function (item, index) {
      var node = template.content.cloneNode(true);
      var link = node.querySelector("[data-palette-link]");
      link.href = item.href;
      node.querySelector("[data-palette-label]").textContent = item.label;
      node.querySelector("[data-palette-group]").textContent = item.group;
      if (index === active) link.setAttribute("data-active", "");
      results.appendChild(node);
    });
  }

  function filter(query) {
    if (!items) return;
    var needle = query.toLowerCase();
    filtered = items
      .filter(function (item) {
        return (
          item.label.toLowerCase().indexOf(needle) >= 0 ||
          item.group.toLowerCase().indexOf(needle) >= 0
        );
      })
      .slice(0, 8);
    active = 0;
    render();
  }

  function open() {
    overlay.setAttribute("data-open", "");
    input.value = "";
    load().then(function () {
      filter("");
    });
    input.focus();
  }

  function close() {
    overlay.removeAttribute("data-open");
  }

  document.addEventListener("keydown", function (event) {
    if ((event.metaKey || event.ctrlKey) && (event.key === "k" || event.key === "K")) {
      event.preventDefault();
      open();
    } else if (event.key === "Escape") {
      close();
    }
  });

  input.addEventListener("input", function () {
    filter(input.value);
  });

  input.addEventListener("keydown", function (event) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      active = Math.min(active + 1, filtered.length - 1);
      render();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      active = Math.max(active - 1, 0);
      render();
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (filtered[active]) window.location.href = filtered[active].href;
    }
  });

  overlay.addEventListener("click", function (event) {
    if (event.target === overlay) close();
  });
})();

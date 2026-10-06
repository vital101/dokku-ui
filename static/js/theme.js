(function () {
  var match = document.cookie.match(/(?:^|;\s*)theme=(light|dark)/);
  var theme = match ? match[1] : "dark";
  document.documentElement.setAttribute("data-theme", theme);
  window.addEventListener("DOMContentLoaded", function () {
    var buttons = document.querySelectorAll("[data-theme-toggle]");
    for (var i = 0; i < buttons.length; i++) {
      buttons[i].addEventListener("click", function () {
        var current = document.documentElement.getAttribute("data-theme");
        var next = current === "light" ? "dark" : "light";
        document.cookie =
          "theme=" + next + ";path=/;max-age=31536000;samesite=lax";
        document.documentElement.setAttribute("data-theme", next);
      });
    }
  });
})();

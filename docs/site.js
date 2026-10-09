// brie site: a frosted nav on scroll and copy buttons. Nothing else needs script.
(function () {
  var nav = document.querySelector(".nav");
  if (nav) {
    var ticking = false;
    var update = function () {
      nav.classList.toggle("is-scrolled", window.scrollY > 24);
      ticking = false;
    };
    window.addEventListener(
      "scroll",
      function () {
        if (!ticking) {
          ticking = true;
          window.requestAnimationFrame(update);
        }
      },
      { passive: true }
    );
    update();
  }

  // Copy buttons: silent success, the label says what happened.
  document.querySelectorAll("[data-copy]").forEach(function (button) {
    var label = button.textContent;
    var timer;
    button.addEventListener("click", function () {
      var source = document.getElementById(button.getAttribute("data-copy"));
      if (!source || !navigator.clipboard) return;
      navigator.clipboard.writeText(source.innerText.trim()).then(
        function () {
          button.textContent = "Copied";
          button.setAttribute("data-state", "copied");
          clearTimeout(timer);
          timer = setTimeout(function () {
            button.textContent = label;
            button.removeAttribute("data-state");
          }, 1400);
        },
        function () {
          button.textContent = "Press ⌘C";
        }
      );
    });
  });
})();

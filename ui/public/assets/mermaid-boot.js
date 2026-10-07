// Draws every ```mermaid block the app renders, wherever it renders one.
//
// The contract with the Rust side is one line of markup: `md_to_html` turns a mermaid fence into
// `<pre class="mermaid">…</pre>` instead of a code block, and this file turns that into a diagram. Nothing
// else crosses — no eval, no interop, no hook to remember at each place markdown is shown.
//
// A MutationObserver rather than a call per screen, because markdown arrives at four of them and always
// LATE: a document's text is fetched after its row is clicked, a task's dialog mounts before its body is
// known. Watching the DOM catches all of it, including the arrivals nobody thought to hook.
//
// The library itself is 3.5Mb and is fetched ONLY once a diagram is actually on the page — most sessions
// never open one, and a board that took a megabyte longer to start for a feature nobody used that day would
// be a bad trade.
(function () {
  "use strict";

  // Versioned in the name so a new mermaid is a new URL: this path lives inside a .js file, where the
  // cache-busting that rewrites index.html cannot reach it.
  var SRC = "/assets/mermaid-11.16.0.min.js";

  // Ours, not mermaid's `data-processed`: the claim has to be made BEFORE the library is fetched, or every
  // mutation during the fetch would queue the same block again.
  var CLAIMED = "data-mermaid-claimed";
  var SELECTOR = "pre.mermaid:not([" + CLAIMED + "])";

  var loading = null;
  var scheduled = false;

  function load() {
    if (loading) {
      return loading;
    }

    loading = new Promise(function (resolve, reject) {
      var tag = document.createElement("script");
      tag.src = SRC;

      tag.onload = function () {
        // `startOnLoad` off because the diagrams are drawn by us, when they appear, rather than once at
        // page load — by which time there are none. `strict` keeps html labels sanitised: the diagrams are
        // written by agents, and they are drawn on the origin that holds the session.
        window.mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          theme: "default",
        });

        resolve();
      };

      tag.onerror = reject;
      document.head.appendChild(tag);
    });

    return loading;
  }

  function draw() {
    scheduled = false;

    var nodes = document.querySelectorAll(SELECTOR);

    if (nodes.length === 0) {
      return;
    }

    for (var i = 0; i < nodes.length; i++) {
      nodes[i].setAttribute(CLAIMED, "1");
    }

    load()
      .then(function () {
        // `suppressErrors` so one diagram that does not parse becomes one error box rather than an
        // exception that leaves the rest of the page's diagrams undrawn.
        window.mermaid.run({ nodes: nodes, suppressErrors: true });
      })
      .catch(function () {
        // The library did not load. Give the blocks back, so the next thing that changes the page tries
        // again — and until then they read as the source they are.
        for (var j = 0; j < nodes.length; j++) {
          nodes[j].removeAttribute(CLAIMED);
        }
      });
  }

  // One pass per frame at most. Drawing a diagram mutates the DOM, which wakes the observer that asked for
  // it — without this, a page with a diagram would loop.
  function schedule() {
    if (scheduled) {
      return;
    }

    scheduled = true;
    requestAnimationFrame(draw);
  }

  function start() {
    new MutationObserver(schedule).observe(document.body, {
      childList: true,
      subtree: true,
    });

    // The first look, for markup that was already there when this ran.
    schedule();
  }

  if (document.body) {
    start();
  } else {
    document.addEventListener("DOMContentLoaded", start);
  }
})();

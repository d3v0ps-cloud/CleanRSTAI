/* CleanMarkdown: a small, safe Markdown-to-HTML renderer for chat replies.
 * Safety model: every piece of text is HTML-escaped before markup is added, so
 * model output can never inject tags or attributes. Links are limited to
 * http:, https: and mailto:; images render as links. No dependencies. */
var CleanMarkdown = (function () {
  "use strict";

  var MAX_BLOCK_DEPTH = 8;   // nested blockquotes
  var MAX_INLINE_DEPTH = 16; // nested emphasis and link labels

  var ESC = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };

  function escapeHtml(s) {
    return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) { return ESC[c]; });
  }

  // Block patterns
  var FENCE_OPEN = /^ {0,3}(`{3,})([^`]*)$/;
  var FENCE_CLOSE = /^ {0,3}(`{3,})[ \t]*$/;
  var HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?[ \t]*$/;
  var HR = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
  var BLOCKQUOTE = /^ {0,3}>/;
  var ITEM = /^( *)([-*+]|\d{1,9}[.)])[ \t]+(.*)$/;
  var TABLE_SEP = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;
  var LANG = /^[A-Za-z0-9_+-]+$/;

  // Inline patterns, matched with sticky flags at the current index
  var LINK_TAIL = /\(\s*([^()\s]*)\s*\)/y;   // the "(url)" part after a "]"
  var BRACKET_STOP = /[\]\n]/g;                 // ends a link label
  var AUTOLINK = /https?:\/\/[^\s<>"'`\[\]]+/iy;
  var SAFE_URL = /^(https?:\/\/|mailto:)/i;

  function isBlank(s) { return /^\s*$/.test(s); }

  function isWord(c) { return c !== "" && /[\p{L}\p{N}]/u.test(c); }

  function count(s, c) { return s.split(c).length - 1; }

  // ---- Inline rendering ----------------------------------------------------

  function inline(raw, inLink, depth) {
    if (depth > MAX_INLINE_DEPTH) return escapeHtml(raw);
    var out = "";
    var buf = "";            // pending plain text, escaped when flushed
    var i = 0;
    var m;
    // Scan results that are reused for later openers, so unmatched openers
    // cost O(1) each instead of a rescan to the end of the text.
    var memo = { noCloser: {}, noCode: {}, stopFrom: -1, stopAt: -1, tails: {} };

    function flush() { out += escapeHtml(buf); buf = ""; }

    while (i < raw.length) {
      var ch = raw.charAt(i);

      // Backslash escapes an ASCII punctuation character
      if (ch === "\\" && /[!-\/:-@\[-`{-~]/.test(raw.charAt(i + 1))) {
        buf += raw.charAt(i + 1);
        i += 2;
        continue;
      }

      // Inline code: a backtick run closed by a run of the same length
      if (ch === "`") {
        var n = 0;
        while (raw.charAt(i + n) === "`") n++;
        var close = findCodeClose(raw, i + n, n, memo);
        if (close === -1) {
          buf += raw.substr(i, n);
          i += n;
          continue;
        }
        var code = raw.slice(i + n, close);
        if (/^ .*\S.* $/.test(code)) code = code.slice(1, -1);
        flush();
        out += "<code>" + escapeHtml(code) + "</code>";
        i = close + n;
        continue;
      }

      // Links and images (images become links carrying their alt text)
      if (ch === "[" || (ch === "!" && raw.charAt(i + 1) === "[")) {
        m = linkAt(raw, i, memo);
        if (m) {
          if (!SAFE_URL.test(m.url)) {         // unsafe scheme: plain text
            buf += raw.substr(i, m.len);
            i += m.len;
            continue;
          }
          flush();
          var label = m.label || m.url;
          var text = m.image || inLink ? escapeHtml(label) : inline(label, true, depth + 1);
          out += '<a href="' + escapeHtml(m.url) +
            '" target="_blank" rel="noopener noreferrer">' + text + "</a>";
          i += m.len;
          continue;
        }
      }

      // Bare http(s) URLs
      if ((ch === "h" || ch === "H") && !inLink) {
        AUTOLINK.lastIndex = i;
        m = AUTOLINK.exec(raw);
        if (m) {
          var url = trimUrl(m[0]);
          flush();
          out += '<a href="' + escapeHtml(url) +
            '" target="_blank" rel="noopener noreferrer">' + escapeHtml(url) + "</a>";
          i += url.length;
          continue;
        }
      }

      // Emphasis: **strong**, *em*, __strong__, _em_, ~~strike~~
      if (ch === "*" || ch === "_" || ch === "~") {
        var d = raw.substr(i, 2) === ch + ch ? ch + ch : ch;
        if (ch === "~" && d !== "~~") {
          buf += ch;
          i++;
          continue;
        }
        if (openerOk(raw, i, d)) {
          var end = findCloser(raw, i + d.length, d, memo);
          if (end !== -1) {
            flush();
            var tag = ch === "~" ? "del" : d.length === 2 ? "strong" : "em";
            out += "<" + tag + ">" + inline(raw.slice(i + d.length, end), inLink, depth + 1) +
              "</" + tag + ">";
            i = end + d.length;
            continue;
          }
        }
        buf += ch;   // no matching closer: literal
        i++;
        continue;
      }

      buf += ch;
      i++;
    }
    flush();
    return out;
  }

  // Closing backtick run of exactly n backticks at or after `from`, or -1.
  // A search that fails from `from` also fails from any later position.
  function findCodeClose(raw, from, n, memo) {
    if (memo.noCode[n] !== undefined && from >= memo.noCode[n]) return -1;
    var p = raw.indexOf("`", from);
    while (p !== -1) {
      var q = p;
      while (raw.charAt(q) === "`") q++;
      if (q - p === n) return p;
      p = raw.indexOf("`", q);
    }
    memo.noCode[n] = memo.noCode[n] === undefined ? from : Math.min(memo.noCode[n], from);
    return -1;
  }

  // Index of the next "]" or newline at or after `from` (or raw.length).
  // Valid for any position up to the cached stop, since nothing stops in between.
  function nextBracketStop(raw, from, memo) {
    if (from >= memo.stopFrom && from <= memo.stopAt) return memo.stopAt;
    BRACKET_STOP.lastIndex = from;
    var m = BRACKET_STOP.exec(raw);
    memo.stopFrom = from;
    memo.stopAt = m ? m.index : raw.length;
    return memo.stopAt;
  }

  // Link or image starting at i (a "[", or "!" before "["), or null.
  // Label: no "]" or newline. Destination: "(" ws url ws ")".
  function linkAt(raw, i, memo) {
    var image = raw.charAt(i) === "!";
    var open = image ? i + 1 : i;
    var stop = nextBracketStop(raw, open + 1, memo);
    if (raw.charAt(stop) !== "]") return null;
    var tail = linkTail(raw, stop, memo);
    if (!tail) return null;
    return { len: tail.end - i, url: tail.url, label: raw.slice(open + 1, stop), image: image };
  }

  // The "(url)" after the "]" at `stop`. Memoized, since several openers can share one "]".
  function linkTail(raw, stop, memo) {
    if (stop in memo.tails) return memo.tails[stop];
    var r = null;
    if (raw.charAt(stop + 1) === "(") {
      LINK_TAIL.lastIndex = stop + 1;
      var m = LINK_TAIL.exec(raw);
      if (m) r = { url: m[1], end: stop + 1 + m[0].length };
    }
    memo.tails[stop] = r;
    return r;
  }

  // An opener must be followed by non-space; underscores must not follow a word character
  function openerOk(raw, i, d) {
    var after = raw.charAt(i + d.length);
    if (after === "" || /\s/.test(after)) return false;
    if (d.charAt(0) === "_" && isWord(raw.charAt(i - 1))) return false;
    return true;
  }

  // A closer must be preceded by non-space and not be part of a longer run.
  // A search that fails from `from` also fails from any later position (memoized per delimiter).
  function findCloser(raw, from, d, memo) {
    if (memo.noCloser[d] !== undefined && from >= memo.noCloser[d]) return -1;
    for (var j = from; j + d.length <= raw.length; j++) {
      if (raw.substr(j, d.length) !== d) continue;
      if (j === from || /\s/.test(raw.charAt(j - 1))) continue;
      if (d.charAt(0) === "_" && isWord(raw.charAt(j + d.length))) continue;
      if (d.length === 1 && raw.charAt(j + 1) === d) continue;
      return j;
    }
    memo.noCloser[d] = memo.noCloser[d] === undefined ? from : Math.min(memo.noCloser[d], from);
    return -1;
  }

  // Drop sentence punctuation (and unbalanced ")") from the end of an autolink
  function trimUrl(url) {
    var end = url.length;
    var opens = count(url, "(");
    var closes = count(url, ")");
    while (end > 0) {
      var last = url.charAt(end - 1);
      if (".,;:!?".indexOf(last) !== -1) {
        end--;
      } else if (last === ")" && opens < closes) {
        closes--;
        end--;
      } else {
        break;
      }
    }
    return url.slice(0, end);
  }

  function renderLines(lines) {
    return lines.map(function (l) { return inline(l, false, 0); }).join("<br>");
  }

  // ---- Tables --------------------------------------------------------------

  // Split a table row on unescaped pipes, honouring a leading and trailing pipe
  function splitRow(line) {
    var s = line.trim();
    if (s.charAt(0) === "|") s = s.slice(1);
    if (s.slice(-1) === "|" && s.slice(-2) !== "\\|") s = s.slice(0, -1);
    var cells = [];
    var cur = "";
    for (var k = 0; k < s.length; k++) {
      var ch = s.charAt(k);
      if (ch === "\\" && s.charAt(k + 1) === "|") {
        cur += "|";
        k++;
      } else if (ch === "|") {
        cells.push(cur.trim());
        cur = "";
      } else {
        cur += ch;
      }
    }
    cells.push(cur.trim());
    return cells;
  }

  function alignOf(sep) {
    var left = sep.charAt(0) === ":";
    var right = sep.slice(-1) === ":";
    if (left && right) return "center";
    if (right) return "right";
    if (left) return "left";
    return "";
  }

  function isTableStart(lines, i) {
    return i + 1 < lines.length && lines[i].indexOf("|") !== -1 &&
      TABLE_SEP.test(lines[i + 1]) &&
      splitRow(lines[i]).length === splitRow(lines[i + 1]).length;
  }

  function tableCells(row, tag, aligns, cols) {
    var s = "";
    for (var c = 0; c < cols; c++) {
      var a = aligns[c] ? ' align="' + aligns[c] + '"' : "";
      s += "<" + tag + a + ">" + inline(row[c] || "", false, 0) + "</" + tag + ">";
    }
    return s;
  }

  function parseTable(lines, start) {
    var head = splitRow(lines[start]);
    var aligns = splitRow(lines[start + 1]).map(alignOf);
    var cols = head.length;
    var html = '<div class="table-wrap"><table><thead><tr>' +
      tableCells(head, "th", aligns, cols) + "</tr></thead><tbody>";
    var i = start + 2;
    while (i < lines.length && !isBlank(lines[i]) && lines[i].indexOf("|") !== -1) {
      html += "<tr>" + tableCells(splitRow(lines[i]), "td", aligns, cols) + "</tr>";
      i++;
    }
    return { html: html + "</tbody></table></div>", next: i };
  }

  // ---- Lists ---------------------------------------------------------------

  // Items indented by 2+ spaces become children of the previous item (one level)
  function parseList(lines, start) {
    var first = ITEM.exec(lines[start]);
    var ordered = /\d/.test(first[2]);
    var items = [];
    var last = null;
    var i = start;

    while (i < lines.length) {
      var line = lines[i];
      if (isBlank(line)) {
        // A blank line keeps the list open only if another item of the same kind follows
        var k = i + 1;
        while (k < lines.length && isBlank(lines[k])) k++;
        var next = k < lines.length ? ITEM.exec(lines[k]) : null;
        if (!next || /\d/.test(next[2]) !== ordered) break;
        i = k;
        continue;
      }
      var m = ITEM.exec(line);
      if (m) {
        if (/\d/.test(m[2]) !== ordered) break;
        last = { text: m[3], children: [] };
        if (m[1].length >= 2 && items.length) {
          items[items.length - 1].children.push(last);
        } else {
          items.push(last);
        }
        i++;
        continue;
      }
      if (/^\s/.test(line) && last) {   // indented continuation of the last item
        last.text += "\n" + line.trim();
        i++;
        continue;
      }
      break;
    }

    var tag = ordered ? "ol" : "ul";
    var num = parseInt(first[2], 10);
    var html = "<" + tag + (ordered && num !== 1 ? ' start="' + num + '"' : "") + ">";
    items.forEach(function (it) {
      html += "<li>" + renderLines(it.text.split("\n"));
      if (it.children.length) {
        html += "<" + tag + ">" + it.children.map(function (c) {
          return "<li>" + renderLines(c.text.split("\n")) + "</li>";
        }).join("") + "</" + tag + ">";
      }
      html += "</li>";
    });
    return { html: html + "</" + tag + ">", next: i };
  }

  // ---- Blocks --------------------------------------------------------------

  function codeBlock(lang, code) {
    var langAttr = lang ? ' class="language-' + lang + '" data-lang="' + lang + '"' : "";
    return '<div class="code-block"><div class="code-head"><span>' + escapeHtml(lang || "text") +
      '</span><button type="button" class="copy-btn">Copy</button></div><pre><code' + langAttr +
      ">" + escapeHtml(code) + "</code></pre></div>";
  }

  // Does line k start a block that interrupts a paragraph?
  function startsBlock(lines, k) {
    var s = lines[k];
    return FENCE_OPEN.test(s) || HEADING.test(s) || HR.test(s) || BLOCKQUOTE.test(s) ||
      ITEM.test(s) || isTableStart(lines, k);
  }

  function renderBlocks(lines, depth) {
    var out = [];
    var i = 0;
    while (i < lines.length) {
      var line = lines[i];
      if (isBlank(line)) {
        i++;
        continue;
      }
      var m;

      // Fenced code: an unclosed fence runs to the end (replies stream in)
      if ((m = FENCE_OPEN.exec(line))) {
        var open = m[1].length;
        var lang = m[2].trim().split(/\s+/)[0] || "";
        var body = [];
        var j;
        for (j = i + 1; j < lines.length; j++) {
          var c = FENCE_CLOSE.exec(lines[j]);
          if (c && c[1].length >= open) break;
          body.push(lines[j]);
        }
        out.push(codeBlock(LANG.test(lang) ? lang : "", body.join("\n")));
        i = j + 1;
        continue;
      }

      if ((m = HEADING.exec(line))) {
        var tag = "h" + m[1].length;
        var title = (m[2] || "").replace(/[ \t]+#+$/, "");
        out.push("<" + tag + ">" + inline(title, false, 0) + "</" + tag + ">");
        i++;
        continue;
      }

      if (HR.test(line)) {
        out.push("<hr>");
        i++;
        continue;
      }

      if (BLOCKQUOTE.test(line) && depth < MAX_BLOCK_DEPTH) {
        var inner = [];
        for (; i < lines.length && BLOCKQUOTE.test(lines[i]); i++) {
          inner.push(lines[i].replace(/^ {0,3}> ?/, ""));
        }
        out.push("<blockquote>" + renderBlocks(inner, depth + 1) + "</blockquote>");
        continue;
      }

      if (ITEM.test(line)) {
        var list = parseList(lines, i);
        out.push(list.html);
        i = list.next;
        continue;
      }

      if (isTableStart(lines, i)) {
        var table = parseTable(lines, i);
        out.push(table.html);
        i = table.next;
        continue;
      }

      // Paragraph: consecutive lines until a blank line or another block
      var para = [line];
      i++;
      while (i < lines.length && !isBlank(lines[i]) && !startsBlock(lines, i)) {
        para.push(lines[i]);
        i++;
      }
      out.push("<p>" + renderLines(para) + "</p>");
    }
    return out.join("\n");
  }

  function render(text) {
    var src = String(text == null ? "" : text).replace(/\r\n?/g, "\n");
    return renderBlocks(src.split("\n"), 0);
  }

  return { render: render, escapeHtml: escapeHtml };
})();

if (typeof window !== "undefined") window.CleanMarkdown = CleanMarkdown;
if (typeof module !== "undefined") module.exports = CleanMarkdown;

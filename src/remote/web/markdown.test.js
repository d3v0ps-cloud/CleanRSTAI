"use strict";
// Run with: node src/remote/web/markdown.test.js
const assert = require("assert");
const M = require("./markdown.js");

const cases = [];
function t(name, fn) { cases.push([name, fn]); }

const LINK_ATTRS = 'target="_blank" rel="noopener noreferrer"';

// Shared XSS assertions: no live markup from input, no javascript: hrefs, no attribute breakout
function assertSafe(out) {
  assert(!/<script/i.test(out), "contains <script: " + out);
  assert(!/<img/i.test(out), "contains <img: " + out);
  assert(!/<svg/i.test(out), "contains <svg: " + out);
  (out.match(/href="[^"]*"/gi) || []).forEach((h) => {
    assert(!/javascript:/i.test(h), "javascript href: " + h);
  });
  assert(!/href="[^"]*"[^>]*on\w+=/i.test(out), "attribute breakout: " + out);
}

// ---- Escaping and basics ----
t("escapeHtml escapes & < > \" '", () => {
  assert.strictEqual(M.escapeHtml(`&<>"'`), "&amp;&lt;&gt;&quot;&#39;");
});
t("empty and null input render nothing", () => {
  assert.strictEqual(M.render(""), "");
  assert.strictEqual(M.render(undefined), "");
});
t("paragraph", () => {
  assert.strictEqual(M.render("hello"), "<p>hello</p>");
});
t("line breaks inside a paragraph become <br>", () => {
  assert.strictEqual(M.render("a\nb"), "<p>a<br>b</p>");
});
t("headings h2 and h6", () => {
  assert.strictEqual(M.render("## Title"), "<h2>Title</h2>");
  assert.strictEqual(M.render("###### x"), "<h6>x</h6>");
});
t("hash without space is not a heading", () => {
  assert.strictEqual(M.render("#tag"), "<p>#tag</p>");
});

// ---- Inline formatting ----
t("bold with ** and __", () => {
  assert.strictEqual(M.render("**b**"), "<p><strong>b</strong></p>");
  assert.strictEqual(M.render("__b__"), "<p><strong>b</strong></p>");
});
t("italic with * and _", () => {
  assert.strictEqual(M.render("*i*"), "<p><em>i</em></p>");
  assert.strictEqual(M.render("_i_"), "<p><em>i</em></p>");
});
t("strikethrough", () => {
  assert.strictEqual(M.render("~~s~~"), "<p><del>s</del></p>");
});
t("underscores inside words are literal", () => {
  assert.strictEqual(M.render("snake_case_name"), "<p>snake_case_name</p>");
});
t("backslash escapes punctuation", () => {
  assert.strictEqual(M.render("\\*not em\\*"), "<p>*not em*</p>");
});
t("inline code is not formatted and is escaped", () => {
  assert.strictEqual(M.render("`**x** <b>`"), "<p><code>**x** &lt;b&gt;</code></p>");
});

// ---- Links ----
t("http link with escaped & in href and link attributes", () => {
  assert.strictEqual(M.render("[a](https://x.io/?a=1&b=2)"),
    `<p><a href="https://x.io/?a=1&amp;b=2" ${LINK_ATTRS}>a</a></p>`);
});
t("mailto link is allowed", () => {
  assert.strictEqual(M.render("[mail](mailto:a@b.c)"),
    `<p><a href="mailto:a@b.c" ${LINK_ATTRS}>mail</a></p>`);
});
t("bare https autolink, trailing period excluded", () => {
  assert.strictEqual(M.render("see https://example.com."),
    `<p>see <a href="https://example.com" ${LINK_ATTRS}>https://example.com</a>.</p>`);
});
t("image renders as a link with its alt text", () => {
  assert.strictEqual(M.render("![logo](https://x.io/a.png)"),
    `<p><a href="https://x.io/a.png" ${LINK_ATTRS}>logo</a></p>`);
});

// ---- Code ----
t("fenced code block with language", () => {
  assert.strictEqual(M.render("```js\nconst a = 1 < 2;\n```"),
    '<div class="code-block"><div class="code-head"><span>js</span>' +
    '<button type="button" class="copy-btn">Copy</button></div>' +
    '<pre><code class="language-js" data-lang="js">const a = 1 &lt; 2;</code></pre></div>');
});
t("invalid language name is dropped", () => {
  const out = M.render("```<x>\ny\n```");
  assert(out.includes("<code>y</code>"), out);
  assert(!out.includes("language-"), out);
});
t("unclosed fence runs to end of text (streaming)", () => {
  const out = M.render("```\nfoo\nbar");
  assert(out.includes("<code>foo\nbar</code>"), out);
});
t("no inline formatting inside a code block", () => {
  const out = M.render("```\n**x** [a](https://a.io)\n```");
  assert(out.includes("**x** [a](https://a.io)"), out);
  assert(!out.includes("<strong>") && !out.includes("<a "), out);
});

// ---- Lists, quotes, rules, tables ----
t("unordered list with -, * and + markers", () => {
  assert.strictEqual(M.render("- a\n- b\n* c\n+ d"),
    "<ul><li>a</li><li>b</li><li>c</li><li>d</li></ul>");
});
t("ordered list keeps its start number", () => {
  assert.strictEqual(M.render("3. x\n4. y"), '<ol start="3"><li>x</li><li>y</li></ol>');
});
t("one level of nested list by indentation", () => {
  assert.strictEqual(M.render("- a\n  - b\n- c"),
    "<ul><li>a<ul><li>b</li></ul></li><li>c</li></ul>");
});
t("blockquote", () => {
  assert.strictEqual(M.render("> quote"), "<blockquote><p>quote</p></blockquote>");
});
t("horizontal rule", () => {
  assert.strictEqual(M.render("---"), "<hr>");
});
t("table with alignment and inline cells", () => {
  assert.strictEqual(M.render("| a | b |\n|:--|--:|\n| 1 | **2** |"),
    '<div class="table-wrap"><table><thead><tr><th align="left">a</th><th align="right">b</th></tr>' +
    '</thead><tbody><tr><td align="left">1</td><td align="right"><strong>2</strong></td></tr>' +
    "</tbody></table></div>");
});

// ---- XSS ----
t("XSS: <script> tag is shown as text", () => {
  const out = M.render("<script>alert(1)</script>");
  assert.strictEqual(out, "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>");
  assertSafe(out);
});
t("XSS: <img onerror> is shown as text", () => {
  const out = M.render("<img src=x onerror=alert(1)>");
  assert(out.includes("&lt;img src=x onerror=alert(1)&gt;"), out);
  assertSafe(out);
});
t("XSS: [x](javascript:...) is plain text", () => {
  const out = M.render("[x](javascript:alert(1))");
  assert.strictEqual(out, "<p>[x](javascript:alert(1))</p>");
  assertSafe(out);
});
t("XSS: mixed-case JaVaScRiPt scheme is plain text", () => {
  assertSafe(M.render("[x](JaVaScRiPt:alert(1))"));
});
t("XSS: whitespace before javascript: is plain text", () => {
  assertSafe(M.render("[x]( javascript:alert(1))"));
});
t("XSS: data: URL is plain text", () => {
  assertSafe(M.render("[x](data:text/html,<script>alert(1)</script>)"));
  assertSafe(M.render("[x](data:text/html,<b>hi</b>)"));
});
t("XSS: image with javascript: source is plain text", () => {
  const out = M.render("![a](javascript:x)");
  assert.strictEqual(out, "<p>![a](javascript:x)</p>");
  assertSafe(out);
});
t("XSS: <b> inside inline code is escaped", () => {
  const out = M.render("`<b>`");
  assert.strictEqual(out, "<p><code>&lt;b&gt;</code></p>");
  assertSafe(out);
});
t("XSS: quote and onmouseover in link URL cannot break the attribute", () => {
  const out = M.render('[x](https://a.example/"onmouseover=alert)');
  assert(out.includes("&quot;onmouseover=alert"), out);
  assertSafe(out);
});
t("XSS: html inside a code fence is escaped", () => {
  const out = M.render('```html\n<img src=x onerror=alert(1)>\n```');
  assert(out.includes("&lt;img src=x onerror=alert(1)&gt;"), out);
  assertSafe(out);
});
t("XSS: <svg onload> in a heading is escaped", () => {
  const out = M.render("# <svg onload=alert(1)>");
  assert.strictEqual(out, "<h1>&lt;svg onload=alert(1)&gt;</h1>");
  assertSafe(out);
});
t("XSS: raw html anchor with javascript href is escaped", () => {
  const out = M.render('<a href="javascript:x" onclick="y">hi</a>');
  assert(!out.includes("<a "), out);
  assertSafe(out);
});

// ---- Robustness ----
t("deeply nested blockquote markers do not overflow the stack", () => {
  assert.doesNotThrow(() => M.render(">".repeat(5000) + " x"));
});
t("unbalanced emphasis stays literal", () => {
  assert.strictEqual(M.render("a * b"), "<p>a * b</p>");
  assert.strictEqual(M.render("**a"), "<p>**a</p>");
});

let failed = 0;
for (const [name, fn] of cases) {
  try {
    fn();
    console.log("ok   - " + name);
  } catch (err) {
    failed++;
    console.log("FAIL - " + name + "\n       " + err.message.split("\n").join("\n       "));
  }
}
console.log(`\n${cases.length - failed}/${cases.length} passed`);
if (failed) process.exitCode = 1;

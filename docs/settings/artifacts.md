---
title: Artifacts & files
parent: Settings
nav_order: 8
---

# Artifacts & files
{: .no_toc }

The artifact panel, live web pages, the Save as formats, and reading scanned PDFs.

1. TOC
{:toc}

## Artifacts in a side panel

On by default. When a reply makes an [artifact](../using/chats.html#artifacts) (a web page,
drawing or long code), the panel opens beside the chat. When off, open it with **Open in panel**
by its link in the reply, or the panel button at the top right. How web pages show in the panel
is under [General → Web page preview](artifacts.html#web-page-preview).

## Artifacts

How the artifact panel opens is on the [Appearance](appearance.html) page.

### Web page preview

| Setting | Default | Description |
|---|---|---|
| **Show web pages live in the panel** | on | Web page artifacts show in the panel's **Preview** tab in a web view (WebKit on macOS, WebView2 on Windows). On Linux the panel's **Open live preview** button shows the page in a window of its own, when WebKitGTK is installed (see below). Off: the panel shows the code, and **Open in browser** opens the page. |
| **Let pages run their scripts** | on | Off blocks every script in the page. |
| **Let pages load from the network** | off | Off: scripts, styles, fonts, images and requests from the web are blocked, so a page can't send chat content anywhere; what is written into the page itself still works. Pages that load a library from a CDN need this on. |

On Linux the page opens in its own window, with the same settings for scripts and the network.
WebKitGTK is not part of the release files, so install it first: `libwebkit2gtk-4.1-0` on Debian
and Ubuntu, `webkit2gtk4.1` on Fedora, `webkit2gtk-4.1` on Arch. The checkbox's note says whether
it was found. Without it, or without a display, **Open live preview** opens the page in your
browser instead, with a notice the first time.

Links in a previewed page open in your browser, never in the panel or the Linux window.

## Save as files

Which formats the **Save as…** menu of a [reply or artifact](../using/chats.html#saving-as-files)
offers. The menu lists only the checked formats that suit the content.

| Setting | Default | Description |
|---|---|---|
| **Word document (.docx)** | on | Replies and Markdown, code and CSV artifacts can be saved as Word documents. |
| **PDF (.pdf)** | on | Replies and Markdown, code and CSV artifacts can be saved as PDF files on A4 pages. |
| **Excel workbook (.xlsx)** | on | Replies with a table, and CSV and TSV artifacts, can be saved as Excel workbooks: one sheet per table. |

With all three off, no reply or artifact offers **Save as…**.

## OCR for scanned PDFs

A PDF with no text layer (a scan, or a photo of pages) has almost no text when the app reads it.
With **Read scanned PDFs (OCR)** on, its page images are read when the file is attached, and the
text comes back page by page under `--- Page N ---` headings, with a note that it came from OCR.
The reading shows "Reading scanned PDF… (OCR)" until it is done. With it off, such a PDF is
attached with the note "No text layer; turn on OCR in Settings → Artifacts & files to read the
scanned pages."

| Setting | Default | Description |
|---|---|---|
| **Read scanned PDFs (OCR)** | off | Reads the page images of PDFs that have no text layer. |
| **Read with** | A vision model | **A vision model** sends each page image to a provider, one request per page. **Tesseract** runs the `tesseract` program on this computer. |
| **Vision model by** | The chat's provider | The provider profile for the vision model. The model must accept images. |
| **Tesseract program** | `tesseract` | A name on PATH, or the full path of the program. |
| **Languages** | `eng` | Tesseract language codes joined with `+`, such as `eng+deu`. Each must be installed with Tesseract. |
| **Pages per PDF** | 20 (1–200) | Pages past this are not read, and the text says how many were left out. |

Images in encodings this reader doesn't handle (JPEG 2000, CCITT fax, JBIG2, CMYK) are skipped,
with a note in the text. Images that are inside form XObjects are not read.

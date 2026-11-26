package dev.agync.agenthub.tendril

import org.commonmark.ext.gfm.tables.TablesExtension
import org.commonmark.parser.Parser
import org.commonmark.renderer.html.HtmlRenderer

/** Renders a markdown artifact the way the web app's page does: tables on, raw HTML escaped, never run. */
internal object Markdown {
  private val extensions = listOf(TablesExtension.create())
  private val parser = Parser.builder().extensions(extensions).build()
  private val renderer = HtmlRenderer.builder().extensions(extensions).escapeHtml(true).build()

  fun page(title: String, markdown: String): String =
    """<!doctype html><html><head><meta name="viewport" content="width=device-width, initial-scale=1"><meta charset="utf-8">
<title>${title.replace("<", "&lt;")}</title>
<style>body{font:16px/1.6 system-ui,sans-serif;margin:0;padding:4px 56px 24px 16px;color:#151B17;background:#FBFCFA}
h1{font-size:24px;margin:12px 0 8px}h2{font-size:18px;margin:20px 0 6px}pre,code{font-family:monospace;font-size:14px}pre{overflow-x:auto}
table{width:100%;border-collapse:collapse;font-size:14px}th,td{text-align:left;padding:8px 6px 8px 0;border-bottom:1px solid #D3DAD2}
@media(prefers-color-scheme:dark){body{background:#161B18;color:#E4EAE5}th,td{border-color:#29312C}}</style></head>
<body>${renderer.render(parser.parse(markdown))}</body></html>"""
}

const EXPORT_KATEX_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../node_modules/katex/dist/katex.min.js"
));
const EXPORT_MERMAID_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../node_modules/mermaid/dist/mermaid.min.js"
));

fn inline_script_asset(script: &str) -> String {
    script.replace("</script>", "<\\/script>")
}

#[allow(clippy::too_many_arguments)]
pub fn assemble_html(
    title: &str,
    provider_label: &str,
    provider_clr: &str,
    count: u32,
    date: &str,
    file_size: &str,
    messages_html: &str,
    token_summary_html: &str,
    model_html: &str,
    version_html: &str,
    branch_html: &str,
    path_html: &str,
    needs_katex: bool,
    needs_mermaid: bool,
) -> String {
    let katex_js = if needs_katex {
        format!("<script>{}</script>", inline_script_asset(EXPORT_KATEX_JS))
    } else {
        String::new()
    };
    let mermaid_js = if needs_mermaid {
        format!(
            "<script>{}</script>",
            inline_script_asset(EXPORT_MERMAID_JS)
        )
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="generator" content="CC Session — AI Session Explorer">
<meta name="color-scheme" content="light dark">
<link rel="icon" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><text y='.9em' font-size='90'>💬</text></svg>">
<title>{title}</title>
<style>
*,*::before,*::after {{ box-sizing: border-box; }}
:root {{ --bg: #f9fafb; --bg-bubble: #f5f6f8; --text: #1d1d1f; --text2: #6e6e73; --text3: #9ca3af; --border: #e5e7eb; --code-bg: #f3f4f6; --code-fg: #24292e; --inline-code-bg: rgba(0,0,0,0.06); --inline-code-color: #d63384; --user-bg: linear-gradient(135deg, rgba(0,122,255,0.08), rgba(88,86,214,0.06)); --user-border: rgba(0,122,255,0.12); --user-text: #1d1d1f; --user-muted: #6e6e73; --user-link: #007aff; --user-inline-code-bg: rgba(0,122,255,0.08); --user-inline-code-color: #d63384; --user-code-block-bg: rgba(0,0,0,0.065); --user-code-block-fg: #24292e; --user-code-block-border: rgba(0,122,255,0.14); --user-copy-bg: rgba(0,0,0,0.06); --user-copy-hover-bg: rgba(0,0,0,0.12); --user-copy-color: rgba(0,0,0,0.55); --user-copy-hover-color: #1d1d1f; --user-quote-border: rgba(0,122,255,0.18); --user-hr: rgba(0,122,255,0.12); --user-table-head-bg: rgba(0,122,255,0.06); --user-table-head-border: rgba(0,122,255,0.12); --user-table-head-text: #6e6e73; --user-table-cell-border: rgba(0,0,0,0.08); --user-table-row-hover: rgba(0,122,255,0.04); --link: #2563eb; --diff-old: rgba(239,68,68,0.12); --diff-new: rgba(34,197,94,0.12); }}
@media (prefers-color-scheme: dark) {{
  :root {{ --bg: #111; --bg-bubble: #27292f; --text: #e5e5e7; --text2: #a1a1a6; --text3: #636366; --border: rgba(255,255,255,0.1); --code-bg: #22272e; --code-fg: #adbac7; --inline-code-bg: rgba(255,255,255,0.1); --inline-code-color: #f0abfc; --user-bg: linear-gradient(135deg, rgba(10,132,255,0.12), rgba(88,86,214,0.08)); --user-border: rgba(10,132,255,0.2); --user-text: #ffffff; --user-muted: rgba(255,255,255,0.72); --user-link: #b9dbff; --user-inline-code-bg: rgba(255,255,255,0.14); --user-inline-code-color: #fce4ec; --user-code-block-bg: #22272e; --user-code-block-fg: #adbac7; --user-code-block-border: rgba(255,255,255,0.1); --user-copy-bg: rgba(255,255,255,0.08); --user-copy-hover-bg: rgba(255,255,255,0.16); --user-copy-color: rgba(255,255,255,0.78); --user-copy-hover-color: #ffffff; --user-quote-border: rgba(255,255,255,0.22); --user-hr: rgba(255,255,255,0.16); --user-table-head-bg: rgba(255,255,255,0.12); --user-table-head-border: rgba(255,255,255,0.18); --user-table-head-text: rgba(255,255,255,0.84); --user-table-cell-border: rgba(255,255,255,0.16); --user-table-row-hover: rgba(255,255,255,0.06); --link: #8ab4ff; --diff-old: rgba(239,68,68,0.15); --diff-new: rgba(34,197,94,0.15); }}
}}
body {{ font-family: -apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,'Helvetica Neue',Arial,sans-serif; font-size: 15px; line-height: 1.6; color: var(--text); background: var(--bg); margin: 0; padding: 0; }}
.container {{ max-width: 1280px; margin: 0 auto; padding: 32px 24px 64px; }}
.header {{ padding: 40px 0 28px; border-bottom: 1px solid var(--border); margin-bottom: 36px; }}
.header h1 {{ font-size: 1.6em; font-weight: 700; margin: 0 0 16px; line-height: 1.3; }}
.header-meta {{ display: flex; flex-wrap: wrap; gap: 12px; align-items: center; font-size: 0.85em; color: var(--text2); }}
.badge {{ display: inline-block; padding: 2px 10px; border-radius: 12px; font-size: 0.8em; font-weight: 600; color: #fff; }}
.messages {{ display: flex; flex-direction: column; gap: 16px; }}
.msg {{ display: flex; align-items: flex-start; gap: 10px; }}
.msg-user {{ flex-direction: row-reverse; }}
.msg-tool {{ padding-left: 44px; }}
.msg-system {{ justify-content: center; }}
.avatar {{ width: 34px; height: 34px; display: flex; align-items: center; justify-content: center; flex-shrink: 0; margin-top: 4px; }}
.avatar-user {{ color: #007aff; }}
.avatar-assistant {{ color: {provider_clr}; }}
.bubble {{ max-width: 85%; padding: 12px 16px; border-radius: 16px; word-wrap: break-word; overflow-wrap: break-word; box-shadow: 0 8px 24px rgba(15,23,42,0.04); }}
.bubble-user {{ background: var(--user-bg); color: var(--user-text); border: 1px solid var(--user-border); border-bottom-right-radius: 4px; box-shadow: 0 8px 20px rgba(0,122,255,0.08); }}
.bubble-user .ts, .bubble-user .role-label {{ color: var(--user-muted); }}
.bubble-user a {{ color: var(--user-link); }}
.bubble-assistant {{ background: var(--bg-bubble); border: 1px solid var(--border); color: var(--text); border-bottom-left-radius: 4px; }}
.msg-header {{ display: flex; align-items: center; margin-bottom: 4px; gap: 8px; }}
.msg-actions {{ margin-left: auto; }}
.role-label {{ font-size: 0.75em; font-weight: 600; color: var(--text2); }}
.copy-btn {{ background: rgba(255,255,255,0.72); border: 1px solid transparent; cursor: pointer; font-size: 0.8em; padding: 4px; border-radius: 6px; opacity: 0; transition: opacity 0.15s, background 0.15s, color 0.15s; color: var(--text3); backdrop-filter: blur(10px); }}
.bubble:hover .copy-btn {{ opacity: 1; }}
.copy-btn:hover {{ color: var(--text); background: rgba(255,255,255,0.92); }}
.bubble-user .copy-btn {{ color: var(--user-copy-color); background: var(--user-copy-bg); }}
.bubble-user .copy-btn:hover {{ color: var(--user-copy-hover-color); background: var(--user-copy-hover-bg); }}
.ts {{ font-size: 0.7em; color: var(--text3); white-space: nowrap; }}
.msg-body {{ font-size: 0.95em; }}
.msg-body > :first-child {{ margin-top: 0; }}
.msg-body > :last-child {{ margin-bottom: 0; }}
/* Tool blocks */
.tool-block, .tool-block-closed {{ max-width: 90%; background: var(--bg-bubble); border: 1px solid var(--border); border-radius: 10px; font-size: 0.85em; }}
.tool-block-closed {{ padding: 8px 14px; display: flex; align-items: center; gap: 6px; color: var(--text2); }}
.tool-summary {{ padding: 8px 14px; cursor: pointer; color: var(--text2); display: flex; align-items: center; gap: 6px; user-select: none; list-style: none; }}
.tool-summary::-webkit-details-marker {{ display: none; }}
.tool-summary:hover {{ color: var(--text); }}
.tool-icon {{ font-size: 1em; }}
.tool-name {{ font-family: 'SF Mono',Menlo,monospace; font-weight: 600; color: var(--text); }}
.tool-hint {{ color: var(--text3); font-size: 0.9em; overflow-wrap: anywhere; }}
.tool-content {{ padding: 8px 14px; border-top: 1px solid var(--border); }}
.tool-result-detail {{ border-top: 1px solid var(--border); padding-top: 6px; margin-top: 6px; }}
.tool-field {{ display: flex; gap: 8px; padding: 3px 0; font-size: 0.9em; }}
.tool-field-label {{ color: var(--text3); font-size: 0.85em; font-weight: 600; text-transform: uppercase; min-width: 50px; flex-shrink: 0; }}
.tool-field-value {{ font-family: 'SF Mono',Menlo,monospace; color: var(--text); word-break: break-all; }}
.tool-cmd {{ margin: 0; font-family: 'SF Mono',Menlo,monospace; white-space: pre-wrap; color: var(--text); }}
.tool-diff {{ display: flex; border-radius: 4px; overflow: hidden; margin: 4px 0; }}
.tool-diff pre {{ margin: 0; padding: 6px 8px; font-family: 'SF Mono',Menlo,monospace; font-size: 0.88em; line-height: 1.4; white-space: pre-wrap; word-break: break-word; max-height: 200px; overflow-y: auto; flex: 1; }}
.tool-diff-old {{ background: var(--diff-old); }}
.tool-diff-new {{ background: var(--diff-new); }}
.tool-diff-label {{ padding: 6px; font-family: 'SF Mono',Menlo,monospace; font-weight: 700; flex-shrink: 0; }}
.tool-diff-old .tool-diff-label {{ color: #ef4444; }}
.tool-diff-new .tool-diff-label {{ color: #22c55e; }}
.tool-line-diff {{ margin: 6px 0; border: 1px solid var(--border); border-radius: 6px; overflow: auto; max-height: 360px; background: var(--bg); font-family: 'SF Mono',Menlo,monospace; font-size: 0.88em; line-height: 1.45; }}
.tool-diff-line {{ display: grid; grid-template-columns: 42px 42px 20px minmax(0,1fr); min-width: max-content; }}
.tool-diff-line.add {{ background: var(--diff-new); }}
.tool-diff-line.remove {{ background: var(--diff-old); }}
.tool-diff-line.skip {{ background: var(--code-bg); color: var(--text3); }}
.tool-diff-gutter,.tool-diff-marker {{ padding: 1px 6px; color: var(--text3); user-select: none; text-align: right; border-right: 1px solid var(--border); }}
.tool-diff-marker {{ text-align: center; font-weight: 700; }}
.tool-diff-line.add .tool-diff-marker {{ color: #22c55e; }}
.tool-diff-line.remove .tool-diff-marker {{ color: #ef4444; }}
.tool-diff-code {{ padding: 1px 10px; white-space: pre-wrap; word-break: break-word; }}
.tool-output {{ border-top: 1px solid var(--border); padding: 6px 0; font-family: 'SF Mono',Menlo,monospace; font-size: 0.88em; color: var(--text2); white-space: pre-wrap; max-height: 200px; overflow-y: auto; }}
.tool-raw {{ margin: 0; font-size: 0.88em; white-space: pre-wrap; word-break: break-word; color: var(--text2); }}
.system-text {{ font-size: 0.8em; color: var(--text3); text-align: center; padding: 4px 16px; max-width: 70%; }}
.code-block {{ background: var(--code-bg); color: var(--code-fg); border-radius: 10px; border: 1px solid var(--border); padding: 14px 16px; margin: 8px 0; overflow-x: auto; font-family: 'SF Mono',Menlo,monospace; font-size: 0.88em; line-height: 1.5; }}
.code-block code {{ background: none; padding: 0; color: inherit; }}
.bubble-user .code-block {{ background: var(--user-code-block-bg); color: var(--user-code-block-fg); border-color: var(--user-code-block-border); }}
code {{ background: var(--inline-code-bg); color: var(--inline-code-color); padding: 2px 5px; border-radius: 4px; font-family: 'SF Mono',Menlo,monospace; font-size: 0.85em; }}
.bubble-user code {{ background: var(--user-inline-code-bg); color: var(--user-inline-code-color); }}
blockquote {{ border-left: 3px solid var(--border); margin: 8px 0; padding: 2px 12px; color: var(--text2); }}
blockquote p {{ margin: 4px 0; }}
h1, h2, h3, h4, h5, h6 {{ margin: 10px 0 4px; font-weight: 600; }}
h1 {{ font-size: 1.2em; }} h2 {{ font-size: 1.1em; }} h3 {{ font-size: 1.0em; }} h4 {{ font-size: 0.95em; }} h5 {{ font-size: 0.9em; }} h6 {{ font-size: 0.85em; }}
ul, ol {{ margin: 4px 0; padding-left: 22px; }}
li {{ margin: 2px 0; }}
li > p {{ margin: 2px 0; }}
li input[type="checkbox"] {{ margin: 0 8px 0 0; vertical-align: middle; accent-color: #0a84ff; }}
table {{ border-collapse: collapse; margin: 8px 0; font-size: 0.9em; width: 100%; max-width: 100%; display: block; overflow-x: auto; border-radius: 10px; }}
th, td {{ border: 1px solid var(--border); padding: 5px 10px; text-align: left; white-space: nowrap; }}
th {{ background: var(--inline-code-bg); font-weight: 600; }}
a {{ color: var(--link); text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
hr {{ border: none; border-top: 1px solid var(--border); margin: 12px 0; }}
p {{ margin: 4px 0; }}
.bubble-user blockquote {{ border-left-color: var(--user-quote-border); color: var(--user-muted); }}
.bubble-user hr {{ border-top-color: var(--user-hr); }}
.bubble-user th {{ background: var(--user-table-head-bg); border-color: var(--user-table-head-border); color: var(--user-table-head-text); }}
.bubble-user td {{ border-color: var(--user-table-cell-border); color: var(--user-text); }}
.bubble-user tr:hover td {{ background: var(--user-table-row-hover); }}
.footnote-reference {{ margin-left: 0.12em; font-size: 0.78em; line-height: 0; vertical-align: super; }}
.footnote-reference a {{ color: var(--link); text-decoration: none; }}
.footnote-reference a:hover {{ text-decoration: underline; }}
.footnote-definition {{ margin-top: 10px; padding-top: 8px; border-top: 1px solid var(--border); color: var(--text2); display: flex; gap: 8px; }}
.footnote-definition p {{ margin: 0; }}
.footnote-definition-label {{ color: var(--text3); font-size: 0.78em; line-height: 1; padding-top: 0.25em; }}
.bubble-user .footnote-reference a {{ color: var(--user-link); }}
.bubble-user .footnote-definition {{ border-top-color: var(--user-hr); color: var(--user-muted); }}
.bubble-user .footnote-definition-label {{ color: var(--user-muted); }}
.math.math-inline {{ display: inline-block; max-width: 100%; vertical-align: middle; }}
.math.math-display {{ display: block; margin: 10px 0; overflow-x: auto; text-align: center; }}
.math.math-display math {{ margin: 0 auto; }}
.mermaid-export {{ margin: 8px 0; }}
.mermaid-diagram {{ background: var(--code-bg); border: 1px solid var(--border); border-radius: 10px; padding: 14px 16px; overflow-x: auto; }}
.mermaid-diagram svg {{ display: block; max-width: 100%; height: auto; margin: 0 auto; }}
.mermaid-source {{ margin-top: 8px; }}
.mermaid-source summary {{ cursor: pointer; color: var(--text2); font-size: 0.85em; user-select: none; }}
.bubble-user .mermaid-diagram {{ background: var(--user-code-block-bg); border-color: var(--user-code-block-border); }}
.bubble-user .mermaid-source summary {{ color: var(--user-muted); }}
.msg-image {{ margin: 8px 0; }}
.msg-image img {{ border-radius: 8px; border: 1px solid var(--border); }}
.msg-token-row {{ padding-left: 44px; font-size: 0.78em; color: var(--text3); font-variant-numeric: tabular-nums; margin-top: -12px; }}
.cache-read {{ color: #10b981; }}
.tool-plan {{ padding: 4px 0; }}
.plan-step {{ padding: 3px 0; font-size: 0.9em; }}
.plan-icon {{ font-family: monospace; margin-right: 4px; }}
.plan-done {{ color: #22c55e; }}
.plan-active {{ color: var(--text); font-weight: 600; }}
.plan-pending {{ color: var(--text3); }}
.msg-thinking {{ padding-left: 44px; }}
.thinking-block {{ max-width: 90%; background: var(--bg-bubble); border: 1px solid var(--border); border-radius: 10px; font-size: 0.85em; }}
.thinking-summary {{ padding: 8px 14px; cursor: pointer; color: var(--text3); display: flex; align-items: center; gap: 6px; user-select: none; list-style: none; font-style: italic; }}
.thinking-summary::-webkit-details-marker {{ display: none; }}
.thinking-summary:hover {{ color: var(--text2); }}
.thinking-content {{ padding: 8px 14px; border-top: 1px solid var(--border); color: var(--text2); font-size: 0.95em; line-height: 1.6; white-space: pre-wrap; }}
@media print {{
  body {{ background: #fff; font-size: 12px; }}
  .container {{ max-width: 100%; padding: 0; }}
  .bubble-user {{ background: #007aff !important; color: #fff !important; -webkit-print-color-adjust: exact; print-color-adjust: exact; }}
  .code-block {{ background: #f3f4f6 !important; color: #1a1a1a !important; border: 1px solid #ccc; }}
  .tool-block {{ break-inside: avoid; }}
  details[open] > summary {{ display: none; }}
}}
@media (max-width: 600px) {{
  .bubble, .tool-block, .tool-block-closed, .system-text {{ max-width: 95%; }}
  .container {{ padding: 12px 8px 48px; }}
  .header h1 {{ font-size: 1.2em; }}
}}
/* Lightbox */
.lightbox {{ display: none; position: fixed; inset: 0; background: rgba(0,0,0,0.85); z-index: 9999; justify-content: center; align-items: center; cursor: zoom-out; }}
.lightbox.open {{ display: flex; }}
.lightbox img {{ max-width: 92vw; max-height: 92vh; border-radius: 8px; object-fit: contain; }}
</style>
</head>
<body>
<div class="container">
  <div class="header">
    <h1>{title}</h1>
    <div class="header-meta">
      <span class="badge" style="background:{provider_clr}">{provider_label}</span>
      {path_html}
      <span>💬 {count} messages</span>
      <span>📅 {date}</span>
      <span>📦 {file_size}</span>
      {token_summary_html}
      {model_html}
      {version_html}
      {branch_html}
    </div>
  </div>
  <div class="messages">
{messages_html}
  </div>
</div>
<div class="lightbox" id="lightbox" onclick="closeLightbox()"><img id="lightbox-img" src="" alt="Preview"></div>
{katex_js}
{mermaid_js}
<script>
function openLightbox(src){{document.getElementById('lightbox-img').src=src;document.getElementById('lightbox').classList.add('open')}}
function closeLightbox(){{document.getElementById('lightbox').classList.remove('open')}}
document.addEventListener('keydown',function(e){{if(e.key==='Escape')closeLightbox()}})
function copyMsg(id){{var el=document.getElementById(id);if(!el)return;var body=el.querySelector('.msg-body');if(!body)return;navigator.clipboard.writeText(body.innerText).then(function(){{var btn=el.querySelector('.copy-btn');if(btn){{btn.textContent='✅';setTimeout(function(){{btn.textContent='📋'}},1500)}}}})}}
function renderExportMath(){{
  if(!window.katex)return;
  document.querySelectorAll('.math.math-inline,.math.math-display').forEach(function(el){{
    var tex=el.textContent||'';
    if(!tex.trim())return;
    try{{
      window.katex.render(tex,el,{{displayMode:el.classList.contains('math-display'),throwOnError:false,output:'mathml'}});
    }}catch(err){{
      console.warn('KaTeX export render failed:',err);
    }}
  }});
}}
async function renderExportMermaid(){{
  if(!window.mermaid)return;
  var isDark=window.matchMedia&&window.matchMedia('(prefers-color-scheme: dark)').matches;
  window.mermaid.initialize({{startOnLoad:false,theme:isDark?'dark':'default',securityLevel:'strict',fontFamily:'ui-monospace, SFMono-Regular, Menlo, monospace'}});
  var blocks=Array.from(document.querySelectorAll('pre.code-block > code.language-mermaid'));
  for(var i=0;i<blocks.length;i++){{
    var codeEl=blocks[i];
    var pre=codeEl.parentElement;
    if(!pre)continue;
    var source=codeEl.textContent||'';
    try{{
      var id='export-mermaid-'+i;
      var result=await window.mermaid.render(id,source);
      var wrapper=document.createElement('div');
      wrapper.className='mermaid-export';
      var diagram=document.createElement('div');
      diagram.className='mermaid-diagram';
      diagram.innerHTML=result.svg;
      wrapper.appendChild(diagram);
      var details=document.createElement('details');
      details.className='mermaid-source';
      var summary=document.createElement('summary');
      summary.textContent='Source';
      details.appendChild(summary);
      details.appendChild(pre.cloneNode(true));
      wrapper.appendChild(details);
      pre.replaceWith(wrapper);
    }}catch(err){{
      console.warn('Mermaid export render failed:',err);
    }}
  }}
}}
document.addEventListener('DOMContentLoaded',function(){{
  renderExportMath();
  void renderExportMermaid();
}})
</script>
</body>
</html>"#,
        title = title,
        provider_clr = provider_clr,
        provider_label = provider_label,
        count = count,
        date = date,
        file_size = file_size,
        messages_html = messages_html,
        token_summary_html = token_summary_html,
        model_html = model_html,
        version_html = version_html,
        branch_html = branch_html,
        path_html = path_html,
        katex_js = katex_js,
        mermaid_js = mermaid_js,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn assemble(needs_katex: bool, needs_mermaid: bool) -> String {
        assemble_html(
            "My Session",
            "Claude Code",
            "#d97757",
            7,
            "2026-01-02",
            "12 KB",
            "<div class=\"msg\">BODY</div>",
            "<span>TOKENS</span>",
            "<span>MODEL</span>",
            "<span>VERSION</span>",
            "<span>BRANCH</span>",
            "<span>PATH</span>",
            needs_katex,
            needs_mermaid,
        )
    }

    #[test]
    fn inline_script_asset_neutralises_closing_script_tags() {
        // A nested </script> inside an inlined asset would prematurely close
        // the host <script> tag; it must be escaped to <\/script>.
        let input = "before </script> after";
        assert_eq!(inline_script_asset(input), r"before <\/script> after");
    }

    #[test]
    fn assemble_html_substitutes_all_placeholders() {
        let html = assemble(false, false);
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("<title>My Session</title>"));
        assert!(html.contains("<h1>My Session</h1>"));
        // Provider color flows into both the badge background and avatar rule.
        assert!(html.contains(r#"style="background:#d97757""#));
        assert!(html.contains(".avatar-assistant { color: #d97757; }"));
        assert!(html.contains(">Claude Code</span>"));
        assert!(html.contains("💬 7 messages"));
        assert!(html.contains("📅 2026-01-02"));
        assert!(html.contains("📦 12 KB"));
        // The injected fragment blocks are placed verbatim.
        assert!(html.contains("<div class=\"msg\">BODY</div>"));
        assert!(html.contains("<span>TOKENS</span>"));
        assert!(html.contains("<span>MODEL</span>"));
        assert!(html.contains("<span>VERSION</span>"));
        assert!(html.contains("<span>BRANCH</span>"));
        assert!(html.contains("<span>PATH</span>"));
        assert!(html.trim_end().ends_with("</html>"));
    }

    #[test]
    fn assemble_html_inlines_asset_bundles_only_when_requested() {
        // The static template body already mentions `window.katex` /
        // `window.mermaid` in its runtime glue, so we can't assert on those
        // names. Instead, the multi-hundred-KB minified bundles inflate the
        // document size only when the corresponding flag is set.
        let bare = assemble(false, false);
        let with_katex = assemble(true, false);
        let with_both = assemble(true, true);

        // Inlining KaTeX adds far more than a few KB.
        assert!(
            with_katex.len() > bare.len() + 50_000,
            "katex bundle should inflate output"
        );
        // Adding Mermaid on top inflates further still.
        assert!(
            with_both.len() > with_katex.len() + 50_000,
            "mermaid bundle should inflate output"
        );
    }
}

#!/usr/bin/env python3
"""Render the twelve quote-verified findings on claims that later graded clean."""
import json, html
from pathlib import Path

HERE = Path(__file__).parent
D = json.loads((HERE / "twelve.json").read_text())
e = html.escape

# My reading of each, after reading all twelve against their claims. Grouped by
# what the finding actually is, because the three groups imply different fixes.
VERDICT = {
    ("tet28-modification-target-census.md", "C5"): ("right", "The claim says <em>no</em> surface lets an author type an extent. The evidence covers one module's surface. That is a real universal over a population the capture samples."),
    ("tet28-modification-target-census.md", "C7"): ("arguable", "&ldquo;Refusal rather than report is the right disposition&rdquo; is a design decision, not a description. The verifier answered it with how the code classifies things today, which is beside the point &mdash; but the claim did invite it by asserting rightness in the same breath as a mechanism."),
    ("tet28-modification-target-census.md", "C8"): ("proposal", "The list <em>gains</em> an entry &mdash; that is what the design proposes to add. The verifier read a proposal as a description of the current list and reported the absence as a contradiction."),
    ("tet29-imported-mechanism-premises.md", "C3"): ("right", "The claim asserts a verbatim-substring refusal on premise text. The cited evidence shows a declaration-triggered refusal and never touches the substring check. The claim outruns its citation."),
    ("tet29-imported-mechanism-premises.md", "C8"): ("sufficiency", "All three findings say the captured material &ldquo;does not touch&rdquo; the subject. That is insufficiency in scope&rsquo;s clothing &mdash; exactly what the new prompt was written to exclude, leaking through on a claim that is mostly argument."),
    ("tet29-imported-mechanism-premises.md", "C11"): ("proposal", "The <code>## Transplants</code> section does not exist in <code>compose.rs</code> because <strong>this design is what adds it</strong>. Reported as a contradiction against code the design intends to change."),
    ("tet46-look-never-returns-tetel-content.md", "C2"): ("right", "Both are sharp. The claim names &ldquo;the grep that <code>look_grep</code> invokes&rdquo; on evidence that ran <code>grep --version</code> at a shell and never inspected <code>look_grep</code>; and &ldquo;at any depth&rdquo; rests on a two-depth fixture."),
    ("tet47-ground-what-is-owed.md", "C14"): ("right", "&ldquo;Terminates on any input&rdquo; against evidence covering one citation cycle. Textbook universal overreach, and the kind an attacker pass hunts for."),
    ("tet47-ground-what-is-owed.md", "C20"): ("right", "The claim compares check 5&rsquo;s traversal against the owed closure&rsquo;s behaviour; the capture shows only the first. The comparison&rsquo;s second half is uncited."),
    ("tet56-bounded-grep-return.md", "C8"): ("right", "The strongest of the twelve, and it reads as a genuine numeric conflict: seven censuses called bounded under a 32,768-byte bound, against captured counts of 45,210&ndash;162,896. Either the claim means &ldquo;would be bounded once the fix lands&rdquo; &mdash; in which case it is a proposal read as a description again &mdash; or it is simply wrong."),
    ("tet61-residue-scoped-acknowledgement.md", "C3"): ("right", "&ldquo;Every older build&rdquo; against one build at one pin. Same shape as tet47 C14."),
    ("tet61-residue-scoped-acknowledgement.md", "C19"): ("right", "Reads as a real contradiction: the claim says the log records &ldquo;not the block timestamp&rdquo;, and the implementation records each event&rsquo;s timestamp and derives the block timestamp from that history."),
}
LABEL = {
    "right": ("The verifier looks right", "The claim does outrun its evidence. If this is correct, the grounding pass that graded it <code>supports</code> missed it."),
    "arguable": ("Arguable", "The finding is not wrong about the code, but it answers a design decision with a description of current behaviour."),
    "proposal": ("A proposal read as a description", "The claim describes what <em>this design will build</em>. The verifier compared it against code the design exists to change, and called the difference a contradiction."),
    "sufficiency": ("Insufficiency, leaking through", "The prompt rules &ldquo;the evidence does not establish this&rdquo; out as a finding. It got through anyway, on a claim that is mostly argument."),
}
ORDER = ["right", "proposal", "sufficiency", "arguable"]


def mark(prop, clause):
    """Show the claim with its flagged clause marked, where the clause is verbatim."""
    c = (clause or "").strip()
    if c and c in prop:
        i = prop.index(c)
        return e(prop[:i]) + "<mark>" + e(prop[i:i + len(c)]) + "</mark>" + e(prop[i + len(c):])
    return e(prop)


groups = {k: [] for k in ORDER}
for x in D:
    kind, why = VERDICT[(x["memo"], x["id"])]
    groups[kind].append((x, why))

tally = "".join(
    f'<div class="tally t-{k}"><span class="n">{len(groups[k])}</span>'
    f'<span class="k">{LABEL[k][0]}</span></div>' for k in ORDER)

cards = ""
for k in ORDER:
    if not groups[k]:
        continue
    cards += f'<section class="group"><h2 class="g-{k}">{LABEL[k][0]}</h2><p class="glede">{LABEL[k][1]}</p>'
    for x, why in groups[k]:
        fs = ""
        for f in x["findings"]:
            fs += f"""
      <div class="finding">
        <div class="fkind {f['kind']}">{e(f['kind'])}</div>
        <div class="fbody">
          <div class="lab">Flagged clause</div>
          <p class="clause">{e(f['clause'])}</p>
          <div class="lab">Evidence quoted &mdash; verified verbatim in the captured output</div>
          <pre class="quote">{e((f['evidence'] or '')[:600])}</pre>
          <div class="lab">Why</div>
          <p class="why">{e(f['why'])}</p>
        </div>
      </div>"""
        dropped = ""
        if x["dropped"]:
            dropped = (f'<p class="dropped">{len(x["dropped"])} further finding'
                       f'{"s" if len(x["dropped"]) > 1 else ""} on this claim quoted evidence that could '
                       f'not be found in the captured output, and would be stripped by the quotation check.</p>')
        cards += f"""
    <article class="card">
      <header class="chead">
        <h3>{e(x['memo'].replace('.md',''))} &middot; {e(x['id'])}</h3>
        <span class="graded">later graded {e(', '.join(x['later']))}</span>
      </header>
      <div class="lab">The claim, as it stood at first render &mdash; {x['n_cites']} fact(s) cited</div>
      <p class="prop">{mark(x['prop'], x['findings'][0]['clause'])}</p>
      {fs}
      {dropped}
      <div class="read"><span class="rlab">My read</span><p>{why}</p></div>
    </article>"""
    cards += "</section>"

HTML = f"""<title>Twelve findings on claims that graded clean</title>
<style>
:root {{
  --ground:#F2F3F5; --surface:#FFFFFF; --sunk:#EAECEF; --rule:#D5D9DF;
  --ink:#171B21; --muted:#5A626E; --faint:#828B98;
  --accent:#1F6F6B;
  --right:#1B6B4A; --proposal:#A4442E; --sufficiency:#8A6410; --arguable:#3F5B8C;
  --markbg:#FBE7A2; --markink:#3A2E05;
  --mono:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;
  --sans:system-ui,-apple-system,'Segoe UI',sans-serif;
  --serif:'Iowan Old Style','Palatino Linotype',Palatino,Georgia,serif;
}}
@media (prefers-color-scheme: dark) {{
  :root:not([data-theme="light"]) {{
    --ground:#0F1216; --surface:#161A20; --sunk:#1D222A; --rule:#2B323C;
    --ink:#E4E8ED; --muted:#98A2AF; --faint:#78828F;
    --accent:#5FBDB5;
    --right:#5EC08D; --proposal:#E08A72; --sufficiency:#D5AC55; --arguable:#8AA6D8;
    --markbg:#4A3D10; --markink:#F6E7B4;
  }}
}}
:root[data-theme="dark"] {{
  --ground:#0F1216; --surface:#161A20; --sunk:#1D222A; --rule:#2B323C;
  --ink:#E4E8ED; --muted:#98A2AF; --faint:#78828F;
  --accent:#5FBDB5;
  --right:#5EC08D; --proposal:#E08A72; --sufficiency:#D5AC55; --arguable:#8AA6D8;
  --markbg:#4A3D10; --markink:#F6E7B4;
}}
* {{ box-sizing:border-box; }}
body {{ margin:0; background:var(--ground); color:var(--ink); font-family:var(--sans);
       line-height:1.62; -webkit-font-smoothing:antialiased; }}
.wrap {{ max-width:62rem; margin:0 auto; padding:3rem 1.25rem 5rem;
         display:flex; flex-direction:column; gap:2rem; }}
h1 {{ font-family:var(--serif); font-size:clamp(1.7rem,3.6vw,2.4rem); line-height:1.15;
      margin:0; letter-spacing:-.015em; text-wrap:balance; }}
h2 {{ font-family:var(--serif); font-size:1.35rem; margin:0 0 .3rem; }}
h3 {{ font-family:var(--mono); font-size:.87rem; font-weight:600; margin:0; letter-spacing:-.01em; }}
p {{ margin:0 0 .8rem; }}
.eyebrow {{ font-family:var(--mono); font-size:.7rem; text-transform:uppercase;
            letter-spacing:.14em; color:var(--accent); margin:0 0 .6rem; }}
.lede {{ color:var(--muted); font-size:1.04rem; max-width:62ch; }}
.band {{ background:var(--surface); border:1px solid var(--rule); border-radius:3px; padding:1.4rem 1.35rem; }}
table {{ width:100%; border-collapse:collapse; font-size:.87rem; font-variant-numeric:tabular-nums; }}
th,td {{ text-align:left; padding:.4rem .55rem; border-bottom:1px solid var(--rule); }}
th {{ font-family:var(--mono); font-size:.68rem; text-transform:uppercase;
      letter-spacing:.1em; color:var(--muted); font-weight:600; }}
td.num {{ text-align:right; }}
.tablewrap {{ overflow-x:auto; }}
.tallies {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(10rem,1fr)); gap:.8rem; }}
.tally {{ background:var(--sunk); border:1px solid var(--rule); border-left:3px solid var(--rule);
          border-radius:2px; padding:.7rem .8rem; }}
.tally .n {{ display:block; font-family:var(--mono); font-size:1.6rem; line-height:1.1; }}
.tally .k {{ font-size:.76rem; color:var(--muted); }}
.t-right {{ border-left-color:var(--right); }} .t-right .n {{ color:var(--right); }}
.t-proposal {{ border-left-color:var(--proposal); }} .t-proposal .n {{ color:var(--proposal); }}
.t-sufficiency {{ border-left-color:var(--sufficiency); }} .t-sufficiency .n {{ color:var(--sufficiency); }}
.t-arguable {{ border-left-color:var(--arguable); }} .t-arguable .n {{ color:var(--arguable); }}
.group {{ display:flex; flex-direction:column; gap:1rem; }}
.g-right {{ color:var(--right); }} .g-proposal {{ color:var(--proposal); }}
.g-sufficiency {{ color:var(--sufficiency); }} .g-arguable {{ color:var(--arguable); }}
.glede {{ color:var(--muted); font-size:.94rem; max-width:64ch; margin:0 0 .3rem; }}
.card {{ background:var(--surface); border:1px solid var(--rule); border-radius:3px;
         padding:1.1rem 1.2rem; display:flex; flex-direction:column; gap:.7rem; }}
.chead {{ display:flex; justify-content:space-between; align-items:baseline; gap:1rem; flex-wrap:wrap;
          border-bottom:1px solid var(--rule); padding-bottom:.5rem; }}
.graded {{ font-family:var(--mono); font-size:.7rem; color:var(--faint); }}
.lab {{ font-family:var(--mono); font-size:.65rem; text-transform:uppercase;
        letter-spacing:.11em; color:var(--faint); }}
.prop {{ font-size:.9rem; margin:0; color:var(--ink); }}
mark {{ background:var(--markbg); color:var(--markink); padding:.05em .15em; border-radius:2px; }}
.finding {{ display:flex; gap:.8rem; background:var(--sunk); border:1px solid var(--rule);
            border-radius:2px; padding:.7rem .8rem; }}
.fkind {{ font-family:var(--mono); font-size:.62rem; text-transform:uppercase; letter-spacing:.09em;
          padding:.15rem .4rem; height:fit-content; border:1px solid currentColor; border-radius:2px;
          white-space:nowrap; }}
.fkind.overreaches {{ color:var(--sufficiency); }}
.fkind.contradicts {{ color:var(--proposal); }}
.fbody {{ min-width:0; flex:1; display:flex; flex-direction:column; gap:.25rem; }}
.clause {{ font-family:var(--mono); font-size:.79rem; margin:0 0 .35rem; }}
pre.quote {{ margin:0 0 .35rem; background:var(--surface); border:1px solid var(--rule); border-radius:2px;
             padding:.5rem .6rem; font-family:var(--mono); font-size:.73rem; line-height:1.45;
             white-space:pre-wrap; word-break:break-word; overflow-x:auto; max-height:12rem; overflow-y:auto; }}
.why {{ font-size:.87rem; margin:0; color:var(--muted); }}
.dropped {{ font-size:.8rem; color:var(--faint); font-style:italic; margin:0; }}
.read {{ border-top:1px solid var(--rule); padding-top:.6rem; }}
.read p {{ margin:.2rem 0 0; font-size:.89rem; }}
.rlab {{ font-family:var(--mono); font-size:.65rem; text-transform:uppercase;
         letter-spacing:.11em; color:var(--accent); }}
code {{ font-family:var(--mono); font-size:.9em; background:var(--sunk); padding:.05em .3em; border-radius:2px; }}
:focus-visible {{ outline:2px solid var(--accent); outline-offset:2px; }}
</style>

<div class="wrap">
<header>
  <p class="eyebrow">tetel &mdash; retrodiction test, 2026-08-11</p>
  <h1>Twelve findings on claims that graded clean</h1>
  <p class="lede">A verifier was run over every claim in seven design memos, as each stood at
  first render. These twelve fired on claims that later grading passes graded
  <code>supports</code> &mdash; so the test scores all twelve as false positives. The question
  is whether they are.</p>
</header>

<section class="band">
  <h2>How the test got here</h2>
  <p class="glede">Flag rate on the 62 claims that carry supports and nothing else. Lower is better;
  the design&rsquo;s kill threshold is 7.</p>
  <div class="tablewrap"><table>
    <thead><tr><th>question asked</th><th>evidence shown</th><th class="num">flagged</th><th class="num">rate</th></tr></thead>
    <tbody>
      <tr><td>does the evidence <em>support</em> the claim</td><td>cited facts</td><td class="num">53 / 62</td><td class="num">85%</td></tr>
      <tr><td>does the evidence <em>support</em> the claim</td><td>cited &cup; overlap</td><td class="num">47 / 62</td><td class="num">76%</td></tr>
      <tr><td>does the claim <em>contradict or overreach</em></td><td>cited facts</td><td class="num">23 / 62</td><td class="num">37%</td></tr>
      <tr><td>does the claim <em>contradict or overreach</em></td><td>cited &cup; overlap</td><td class="num">19 / 62</td><td class="num">31%</td></tr>
      <tr><td>&hellip; keeping only findings whose quotation verified</td><td>cited &cup; overlap</td><td class="num">12 / 62</td><td class="num">19%</td></tr>
    </tbody>
  </table></div>
  <p style="margin-top:.9rem; font-size:.9rem; color:var(--muted)">Those last twelve are below.
  Every finding shown quoted a span that was checked, byte for byte, against the captured output
  it claimed to come from &mdash; the check <code>Fact::quotes</code> already performs for transplant
  premises. Across the full run 41% of findings failed that check and would be stripped.</p>
</section>

<section class="band">
  <h2>What the twelve turn out to be</h2>
  <div class="tallies">{tally}</div>
</section>

{cards}

<section class="band">
  <p class="eyebrow">The conclusion</p>
  <p><strong>Eight of twelve</strong> look like real findings on claims that a grounding pass let
  through. If that holds, the residual false-positive rate is not 19% &mdash; the test is scoring the
  verifier against graders who are themselves fallible, and disagreement with a grader is not the
  same as error. On this sample the verifier is right more often than the graders were.</p>
  <p>The dominant remaining <em>defect</em> is one thing, and it is specific: <strong>two of twelve read
  a proposal as a description.</strong> A design memo asserts both what the code does today and what this
  design will make it do, and only the first is checkable against captured bytes. The verifier has no
  way to tell them apart, so it reports the absence of a section the design exists to add as a
  contradiction. That is a prompt problem, not a idea problem, and it is the next thing to fix.</p>
  <p style="margin-bottom:0">The gate still fails: twelve flags against a kill threshold of seven. But
  it began at 53.</p>
</section>
</div>"""

out = HERE / "twelve.html"
out.write_text(HTML)
print(f"wrote {out} ({len(HTML):,} bytes)")
for k in ORDER:
    print(f"  {k:12} {len(groups[k])}")

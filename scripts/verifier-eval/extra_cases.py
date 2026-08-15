"""Cases aimed at the note-versus-extent check specifically.

tetel already does a weak form of this in `scope.rs`: it builds a haystack from
each extent entry's key and label and asks whether the note's text appears in
it, by substring, both directions. That catches a note naming a file nobody
opened only when the file's name happens not to appear anywhere in the extent
string — and it cannot tell "the note mentions report.rs" from "the label
happens to contain the letters report.rs".

Extraction makes the same question precise: pull the paths out of the prose,
compare them against what the extent actually opened.

Same fields as `cases.py`; `flaw=None` means nothing should fire.
"""

EXTRA_CASES = [
    dict(
        id="note-names-a-file-the-extent-never-opened",
        scope="in",
        expect="qualifies",
        flaw="The note concludes about src/report.rs, but the extent only opened src/checks.rs.",
        flaw_markers=["report.rs", "not opened", "outside"],
        proposition=(
            "`machine_check_failed` omits the prose residue from its disjunction, and the "
            "`scope` literal in `src/report.rs` lists it among the human-owed categories instead."
        ),
        extent=["/repo/src/checks.rs (lines 180:196)"],
        output="""180: impl Findings {
188:     pub fn machine_check_failed(&self) -> bool {
189:         !self.grammar.is_empty()
190:             || !self.subset.is_empty()
191:             || !self.out_of_proof.is_empty()
196:     }""",
    ),
    dict(
        id="note-stays-inside-its-extent",
        scope="in",
        expect="supports",
        flaw=None,
        flaw_markers=[],
        proposition=(
            "`machine_check_failed` is an explicit disjunction over the findings fields, and "
            "`out_of_proof` is one of its terms."
        ),
        extent=["/repo/src/checks.rs (lines 180:196)"],
        output="""180: impl Findings {
188:     pub fn machine_check_failed(&self) -> bool {
189:         !self.grammar.is_empty()
190:             || !self.subset.is_empty()
191:             || !self.out_of_proof.is_empty()
196:     }""",
    ),
    dict(
        id="note-names-a-second-file-only-one-was-opened",
        scope="in",
        expect="qualifies",
        flaw="The claim reasons about both compose.rs and ledger.rs; the extent opened only compose.rs.",
        flaw_markers=["ledger.rs", "not opened", "only compose"],
        proposition=(
            "`compose::render` replaces embedded newlines with spaces when it writes a claim into "
            "the ledger table, and `ledger::import` trims each cell on the way back in, so the two "
            "strings differ for any proposition carrying a newline."
        ),
        extent=["/repo/src/compose.rs (lines 310:318)"],
        output="""310: fn ledger_cell(text: &str) -> String {
314:     text.replace('|', "\\\\|").replace('\\n', " ")
318: }""",
    ),
]

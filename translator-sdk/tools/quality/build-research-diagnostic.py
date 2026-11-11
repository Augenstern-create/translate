#!/usr/bin/env python3
"""Build deterministic E0 diagnostic probes for route/error triage.

These are authored templates, not an independently sampled corpus. They are
useful for engineering/route diagnostics only and must never be reported as a
human-reviewed quality benchmark. References are intentionally empty.
"""
from __future__ import annotations
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
nouns = ["man", "woman", "dog", "cat", "boy", "girl", "student", "teacher", "doctor", "friend"]
verbs = ["see", "read", "find", "want", "buy", "open", "close", "send", "help", "watch"]
places = ["school", "house", "station", "market", "office", "park", "library", "kitchen"]
objects = ["book", "telescope", "pen", "bag", "report", "letter", "key", "phone"]
adjs = ["happy", "cold", "ready", "tired", "busy", "quiet", "careful"]
topics = ["AI", "history", "science", "language", "music", "health"]
days = ["Friday", "Monday", "Sunday", "Tuesday"]

templates = [
    ("pp_attachment", "The {n0} saw the {n1} with a {o0}.", True, ["verb_attachment", "noun_attachment"]),
    ("pp_attachment", "She went to the {p0} in the morning.", False, ["location"]),
    ("scope", "I don't {v0} the {n0}.", False, ["predicate_negation"]),
    ("scope", "If it doesn't {v0}, I {v1}.", False, ["condition_negation"]),
    ("relative", "The {n0} who {v0} the {n1} {v1}.", False, ["subject_relative"]),
    ("coordination", "The {n0} and the {n1} {v0}.", False, ["np_coordination"]),
    ("coordination", "I bought two {n0}s and three {n1}s.", False, ["quantity_coordination"]),
    ("realize", "The {n0} is not {a0}.", False, ["predicate_negation"]),
    ("realize", "He went to the {p0}.", False, ["go_place"]),
    ("sense", "I left the bank at five.", True, ["financial_bank", "river_bank"]),
    ("fluency", "The more you {v0}, the better you become.", False, ["correlative"]),
    ("nmt_fidelity", "Please send the {o0} by {d0}.", False, ["deadline"]),
    ("role", "The {n0} gave the {n1} a {o0}.", False, ["ditransitive"]),
    ("realize", "They are {v0}ing the {o0}.", False, ["progressive"]),
    ("sense", "He is reading a book about {t0}.", False, ["topic_pp"]),
    ("fluency", "It is what it is, and that's fine.", True, ["idiom"]),
    ("relative", "The {n0} who saw the {n1} ran.", False, ["relative_patient"]),
    ("pp_attachment", "The {n0} in the {p0} ran.", False, ["noun_attachment"]),
    ("scope", "The {n0} who didn't {v0} the {n1} ran.", False, ["relative_negation"]),
    ("nmt_fidelity", "The result is 42 and the URL is https://example.com/{n0}.", False, ["number_url"]),
]

rows = []
for ti, (cat, tpl, amb, sem) in enumerate(templates, 1):
    for k in range(10):
        vals = {
            "n0": nouns[k % len(nouns)],
            "n1": nouns[(k + 3) % len(nouns)],
            "v0": verbs[(k + ti) % len(verbs)],
            "v1": verbs[(k + ti + 2) % len(verbs)],
            "p0": places[(k + ti) % len(places)],
            "o0": objects[(k + ti) % len(objects)],
            "a0": adjs[(k + ti) % len(adjs)],
            "t0": topics[(k + ti) % len(topics)],
            "d0": days[(k + ti) % len(days)],
        }
        source = tpl.format(**vals)
        rows.append({
            "id": f"probe-{len(rows)+1:04d}",
            "source": source,
            "category": cat,
            "semantic_interpretations": sem,
            "references": [],
            "ambiguity_expected": amb,
            "human_review_status": "pending",
            "split": "diagnostic_probe",
            "provenance": "authored_template_v1",
            "notes": "Synthetic template probe for route/error triage; not a human-reviewed benchmark.",
        })
out = ROOT / "fixtures/research/diagnostic-v2-probes.jsonl"
out.write_text("\n".join(json.dumps(r, ensure_ascii=False) for r in rows) + "\n", encoding="utf-8")
print(len(rows), out)

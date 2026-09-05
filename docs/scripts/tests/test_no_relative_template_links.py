import re
import unittest
from pathlib import Path

# Paths considered template-owned per docs/workflows/template-sync.md
TEMPLATE_PATHS = [
    "docs/workflows",
    "docs/scripts",
    "docs/skills",
    "docs/prompts/shared",
]

# Also include docs/*/_template*.md files and specific files
ROOT = Path(__file__).resolve().parents[2]
ADDITIONAL_FILES = list(ROOT.glob('docs/*/_template*.md')) + [
    ROOT / 'docs' / 'board' / 'README.md',
    ROOT / 'docs' / 'evolution' / 'README.md',
    ROOT / 'docs' / 'context' / 'pulse-store.md',
]

# Regex to find relative links that reference docs/decisions/*.md or docs/versions.md
# Match markdown/link patterns like (../docs/decisions/ADR-...md) or (docs/versions.md)
RELATIVE_DECISIONS_RE = re.compile(r"\((?!https?:\\//|/)[^)]*docs/decisions/[^)\s]*\\.md\)")
RELATIVE_VERSIONS_RE = re.compile(r"\((?!https?:\\//|/)[^)]*docs/versions\\.md\)")


def iter_template_files():
    seen = set()
    for p in TEMPLATE_PATHS:
        base = ROOT / p
        if not base.exists():
            continue
        for f in base.rglob('*'):
            if f.is_file():
                # ignore binary files by extension heuristic
                if f.suffix.lower() in {'.png', '.jpg', '.jpeg', '.gif', '.zip', '.gz'}:
                    continue
                seen.add(f)
    for f in ADDITIONAL_FILES:
        if f and f.exists() and f.is_file():
            seen.add(f)
    # exclude this test file itself
    this_file = Path(__file__).resolve()
    if this_file in seen:
        seen.remove(this_file)
    return sorted(seen)


class TestNoRelativeTemplateLinks(unittest.TestCase):
    def test_no_relative_links_to_framework_decisions_or_versions(self):
        offenders = []
        for f in iter_template_files():
            try:
                text = f.read_text(encoding='utf-8')
            except Exception:
                # if file cannot be decoded as utf-8, skip it
                continue
            lines = text.splitlines()
            for i, line in enumerate(lines, start=1):
                if RELATIVE_DECISIONS_RE.search(line) or RELATIVE_VERSIONS_RE.search(line):
                    offenders.append((str(f.relative_to(ROOT)), i, line.strip()))
        if offenders:
            msg_lines = [
                "Found relative links to docs/decisions/*.md or docs/versions.md in template-owned files:",
            ]
            for path, lineno, snippet in offenders:
                msg_lines.append(f"- {path}:{lineno}: {snippet}")
            self.fail("\n".join(msg_lines))


if __name__ == '__main__':
    unittest.main()

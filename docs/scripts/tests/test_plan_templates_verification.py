import pathlib


def test_templates_have_verification_line():
    root = pathlib.Path(__file__).resolve().parents[2]
    plan_template = (root / "docs" / "plans" / "_template.md").read_text()
    work_unit_template = (root / "docs" / "plans" / "_template-work-unit.md").read_text()

    expected = "Walk every Edge Cases line of the feature spec against the code before ticking Acceptance Criteria."

    assert expected in plan_template, "plan template missing verification line"
    assert expected in work_unit_template, "work-unit template missing verification line"

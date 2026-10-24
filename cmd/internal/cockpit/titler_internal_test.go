package cockpit

import "testing"

// A title is the first line of what the model answered, without what
// models wrap titles in.
func TestCleanTitle(t *testing.T) {
	for answer, want := range map[string]string{
		"Release checklist":                                        "Release checklist",
		`"Release checklist."`:                                     "Release checklist",
		"Title: Release checklist":                                 "Release checklist",
		"**Release checklist**":                                    "Release checklist",
		"# Release checklist\n\nThe user wants a list.":            "Release checklist",
		"<think>a title of a release</think>\n\nRelease checklist": "Release checklist",
		"Название: «Починить тест логина»":                         "Починить тест логина",
		"Titles of the release, v2.0":                              "Titles of the release, v2.0",
		"Title of the release: v2":                                 "Title of the release: v2",
		" \n\n ":                                                   "",
		`"..."`:                                                    "",
	} {
		if got := cleanTitle(answer); got != want {
			t.Errorf("cleanTitle(%q) = %q, want %q", answer, got, want)
		}
	}
}

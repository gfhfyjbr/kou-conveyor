package operation

import "testing"

func TestBoundOutputRejectsMissingTail(t *testing.T) {
	defer func() {
		if got := recover(); got != "bound output: empty tail requires the complete output in head" {
			t.Fatalf("panic = %v, want missing-tail error", got)
		}
	}()
	boundOutput("ab", "", 10, 4, "/capture")
}

func TestBoundOutputWithOptionalPath(t *testing.T) {
	for _, test := range []struct {
		name, head, tail, path, want string
		size                         int64
	}{
		{"complete without path", "abcdef", "", "", "ab...2 bytes truncated...ef", 6},
		{"complete with path", "abcdef", "", "/capture", "ab...2 bytes truncated; complete output in /capture...ef", 6},
		{"partial without path", "ab", "yz", "", "ab...6 bytes truncated...yz", 10},
		{"partial with path", "ab", "yz", "/capture", "ab...6 bytes truncated; complete output in /capture...yz", 10},
		{"partial with newline and backslash", "a\n", "\\z", "", "a\n...96 bytes truncated...\\z", 100},
		{"partial with invalid UTF8", "a\xff", "\xfez", "", "a�...96 bytes truncated...�z", 100},
		{"partial with unicode", "界é", "🙂好", "", "界é...88 bytes truncated...🙂好", 100},
	} {
		t.Run(test.name, func(t *testing.T) {
			got, truncated := boundOutput(test.head, test.tail, test.size, 4, test.path)
			if got != test.want || !truncated {
				t.Fatalf("output = %q, truncated = %t; want %q, true", got, truncated, test.want)
			}
		})
	}
}

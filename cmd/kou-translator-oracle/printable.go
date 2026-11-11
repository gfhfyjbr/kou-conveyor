package main

import (
	"fmt"
	"runtime"
	"strconv"
)

// A development-only source generator; Rust uses the resulting static ranges.
func writePrintable() {
	fmt.Printf("// Generated from strconv.IsPrint in %s; regenerate with kou-translator-oracle --printable.\n", runtime.Version())
	fmt.Println("pub(crate) const PRINTABLE: &[(u32, u32)] = &[")
	for scalar := rune(0); scalar <= 0x10ffff; {
		if !strconv.IsPrint(scalar) {
			scalar++
			continue
		}
		first := scalar
		for scalar < 0x10ffff && strconv.IsPrint(scalar+1) {
			scalar++
		}
		fmt.Printf("    (0x%x, 0x%x),\n", first, scalar)
		scalar++
	}
	fmt.Println("];")
}

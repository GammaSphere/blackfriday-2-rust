// Emits tests/fixtures/go-anchor.txt: measured SanitizedAnchorName output for
// a corpus chosen to exercise the places where Rust's std Unicode predicates
// disagree with Go's.
package main

import (
	"encoding/hex"
	"fmt"

	bf "github.com/russross/blackfriday/v2"
)

func main() {
	cases := []string{
		// the seven cases from the pinned TestSanitizedAnchorName
		"This is a header",
		"This is also          a header",
		"main.go",
		"Article 123",
		"<- Let's try this, shall we?",
		"        ",
		"Hello, 世界",

		// Other_Alphabetic combining marks: Rust's is_alphabetic says yes,
		// Go's IsLetter says no. These are the 11,171.
		"aͅb",   // COMBINING GREEK YPOGEGRAMMENI (Mn)
		"ါ",     // MYANMAR VOWEL SIGN TALL AA (Mc)
		"kְt",   // HEBREW POINT SHEVA (Mn)
		"àb",   // COMBINING GRAVE ACCENT (Mn, not Other_Alphabetic)
		"ั",     // THAI CHARACTER MAI HAN-AKAT
		"xাy",   // BENGALI VOWEL SIGN AA

		// simple vs full lowercase
		"İ",     // LATIN CAPITAL LETTER I WITH DOT ABOVE
		"İstanbul",
		"ß",     // LATIN SMALL LETTER SHARP S
		"ẞ",     // LATIN CAPITAL LETTER SHARP S

		// Nl / No: numbers that are not digits
		"Ⅰ",     // ROMAN NUMERAL ONE (Nl)
		"½",     // VULGAR FRACTION ONE HALF (No)
		"②",     // CIRCLED DIGIT TWO (No)

		// scripts and shaping
		"Ünïcödé Héader",
		"Ωμέγα",
		"Русский текст",
		"日本語の見出し",
		"한국어 제목",
		"العربية",
		"emoji 🎉 header",
		"\U0001F600",

		// structural edge cases
		"", "-", "---", "a", "A", "1",
		"   leading", "trailing   ", "-leading-dash", "trailing-dash-",
		"multiple   spaces   here", "Tabs\tand\nnewlines",
		"!!!", "___", "a_b_c", "a.b.c", "CamelCaseHeader",
		"Header (with parens) [and brackets]",
	}

	for _, s := range cases {
		fmt.Printf("A %s %s\n", hex.EncodeToString([]byte(s)),
			hex.EncodeToString([]byte(bf.SanitizedAnchorName(s))))
	}

	// invalid UTF-8: Go ranges over a string yielding RuneError per bad byte
	invalid := [][]byte{
		{0xff, 0xfe},
		{'a', 0xff, 'b'},
		{0xC3},             // truncated 2-byte sequence
		{0xE2, 0x82},       // truncated 3-byte sequence
		{'a', 0x80, 0x80, 'b'},
		{0xF0, 0x9F, 0x98}, // truncated emoji
	}
	for _, b := range invalid {
		fmt.Printf("A %s %s\n", hex.EncodeToString(b),
			hex.EncodeToString([]byte(bf.SanitizedAnchorName(string(b)))))
	}
}

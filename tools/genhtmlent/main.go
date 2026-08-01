package main

import (
	"encoding/hex"
	"fmt"
	"html"
	"os"
	"sort"
	"strings"
)

func main() {
	populateMaps()
	// --- tables (unchanged) ---
	f, _ := os.Create("html-entities.txt")
	fmt.Fprintf(f, "# longestEntityWithoutSemicolon=%d entity=%d entity2=%d\n",
		longestEntityWithoutSemicolon, len(entity), len(entity2))
	k1 := make([]string, 0, len(entity))
	for k := range entity {
		k1 = append(k1, k)
	}
	sort.Strings(k1)
	for _, k := range k1 {
		fmt.Fprintf(f, "1 %s %04X\n", k, entity[k])
	}
	k2 := make([]string, 0, len(entity2))
	for k := range entity2 {
		k2 = append(k2, k)
	}
	sort.Strings(k2)
	for _, k := range k2 {
		v := entity2[k]
		fmt.Fprintf(f, "2 %s %04X %04X\n", k, v[0], v[1])
	}
	f.Close()

	// --- UnescapeString corpus, using the REAL stdlib html package ---
	g, _ := os.Create("unescape-fixture.txt")
	defer g.Close()

	cases := []string{
		"", "no entities here", "&", "&;", "&#", "&#;", "&#x;", "a & b",
		"&amp;", "&amp", "&ampx", "&lt;&gt;", "x&amp;y", "&aacute;",
		"&notit;", "&noti", "&not", "&notin;",
		"&#38;", "&#x26;", "&#X26;", "&#38", "&#225;", "&#xE1;",
		"&#128;", "&#x80;", "&#153;", "&#159;", "&#144;", "&#157;",
		"&#0;", "&#xD800;", "&#xDFFF;", "&#x110000;", "&#x10FFFF;",
		"&#99999999999;", "&#xFFFFFFFFFF;", "&#4294967296;",
		"&notarealentity;", "&amp;&amp;", "&amp;&lt;&gt;",
		"tail&amp;", "&amp;head", "&NotEqualTilde;", "&NotGreaterGreater;",
		"&nbsp;", "&copy;", "&COPY", "&copy", "&AMP;", "&Gt;", "&gt;",
		"&CounterClockwiseContourIntegral;",
		"http://example.com/?foo=1&bar=2",
		"http://example.com/?a=1&amp;b=2",
		"AT&T", "AT&amp;T", "a&b&c", "&&&", "&&amp;",
		"&#13;", "&#10;", "&#9;", "&#32;",
		"&Aacute", "&zwnj;", "&zscr;",
		"caf&eacute; &amp; bar",
		strings.Repeat("&amp;", 20),
		"&" + strings.Repeat("a", 100) + ";",
		"&#" + strings.Repeat("9", 400) + ";",
	}
	for _, c := range cases {
		fmt.Fprintf(g, "U %s %s\n",
			hex.EncodeToString([]byte(c)),
			hex.EncodeToString([]byte(html.UnescapeString(c))))
	}
	// invalid UTF-8
	for _, b := range [][]byte{
		{0xff, 0xfe},
		{0xff, '&', 'a', 'm', 'p', ';', 0xfe},
		{'a', 0x80, '&', 'l', 't', ';'},
	} {
		fmt.Fprintf(g, "U %s %s\n",
			hex.EncodeToString(b),
			hex.EncodeToString([]byte(html.UnescapeString(string(b)))))
	}
	fmt.Fprintln(os.Stderr, "wrote unescape fixture:", len(cases)+3, "cases")
}

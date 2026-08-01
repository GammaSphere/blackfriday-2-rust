package main

import (
	"fmt"

	bf "github.com/russross/blackfriday/v2"
)

func main() {
	fmt.Println("## Extensions")
	for _, e := range []struct {
		n string
		v bf.Extensions
	}{
		{"NoExtensions", bf.NoExtensions}, {"NoIntraEmphasis", bf.NoIntraEmphasis},
		{"Tables", bf.Tables}, {"FencedCode", bf.FencedCode}, {"Autolink", bf.Autolink},
		{"Strikethrough", bf.Strikethrough}, {"LaxHTMLBlocks", bf.LaxHTMLBlocks},
		{"SpaceHeadings", bf.SpaceHeadings}, {"HardLineBreak", bf.HardLineBreak},
		{"TabSizeEight", bf.TabSizeEight}, {"Footnotes", bf.Footnotes},
		{"NoEmptyLineBeforeBlock", bf.NoEmptyLineBeforeBlock}, {"HeadingIDs", bf.HeadingIDs},
		{"Titleblock", bf.Titleblock}, {"AutoHeadingIDs", bf.AutoHeadingIDs},
		{"BackslashLineBreak", bf.BackslashLineBreak}, {"DefinitionLists", bf.DefinitionLists},
		{"CommonExtensions", bf.CommonExtensions},
	} {
		fmt.Printf("%-24s %10d  0x%05x\n", e.n, e.v, int(e.v))
	}
	fmt.Println("## HTMLFlags")
	for _, e := range []struct {
		n string
		v bf.HTMLFlags
	}{
		{"HTMLFlagsNone", bf.HTMLFlagsNone}, {"SkipHTML", bf.SkipHTML},
		{"SkipImages", bf.SkipImages}, {"SkipLinks", bf.SkipLinks}, {"Safelink", bf.Safelink},
		{"NofollowLinks", bf.NofollowLinks}, {"NoreferrerLinks", bf.NoreferrerLinks},
		{"NoopenerLinks", bf.NoopenerLinks}, {"HrefTargetBlank", bf.HrefTargetBlank},
		{"CompletePage", bf.CompletePage}, {"UseXHTML", bf.UseXHTML},
		{"FootnoteReturnLinks", bf.FootnoteReturnLinks}, {"Smartypants", bf.Smartypants},
		{"SmartypantsFractions", bf.SmartypantsFractions}, {"SmartypantsDashes", bf.SmartypantsDashes},
		{"SmartypantsLatexDashes", bf.SmartypantsLatexDashes},
		{"SmartypantsAngledQuotes", bf.SmartypantsAngledQuotes},
		{"SmartypantsQuotesNBSP", bf.SmartypantsQuotesNBSP}, {"TOC", bf.TOC},
		{"CommonHTMLFlags", bf.CommonHTMLFlags},
	} {
		fmt.Printf("%-24s %10d  0x%05x\n", e.n, e.v, int(e.v))
	}
	fmt.Println("## ListType")
	for _, e := range []struct {
		n string
		v bf.ListType
	}{
		{"ListTypeOrdered", bf.ListTypeOrdered}, {"ListTypeDefinition", bf.ListTypeDefinition},
		{"ListTypeTerm", bf.ListTypeTerm}, {"ListItemContainsBlock", bf.ListItemContainsBlock},
		{"ListItemBeginningOfList", bf.ListItemBeginningOfList}, {"ListItemEndOfList", bf.ListItemEndOfList},
	} {
		fmt.Printf("%-24s %10d  0x%05x\n", e.n, e.v, int(e.v))
	}
	fmt.Println("## CellAlignFlags")
	for _, e := range []struct {
		n string
		v bf.CellAlignFlags
	}{
		{"TableAlignmentLeft", bf.TableAlignmentLeft}, {"TableAlignmentRight", bf.TableAlignmentRight},
		{"TableAlignmentCenter", bf.TableAlignmentCenter},
	} {
		fmt.Printf("%-24s %10d  0x%05x\n", e.n, e.v, int(e.v))
	}
	fmt.Println("## misc")
	fmt.Printf("%-24s %10d\n", "TabSizeDefault", bf.TabSizeDefault)
	fmt.Printf("%-24s %10d\n", "TabSizeDouble", bf.TabSizeDouble)
	fmt.Printf("%-24s %10s\n", "Version", bf.Version)
	fmt.Println("## NodeType ordinals")
	for i, n := range []bf.NodeType{
		bf.Document, bf.BlockQuote, bf.List, bf.Item, bf.Paragraph, bf.Heading,
		bf.HorizontalRule, bf.Emph, bf.Strong, bf.Del, bf.Link, bf.Image, bf.Text,
		bf.HTMLBlock, bf.CodeBlock, bf.Softbreak, bf.Hardbreak, bf.Code, bf.HTMLSpan,
		bf.Table, bf.TableCell, bf.TableHead, bf.TableBody, bf.TableRow,
	} {
		fmt.Printf("%2d %-16s (%d)\n", i, n.String(), int(n))
	}
}

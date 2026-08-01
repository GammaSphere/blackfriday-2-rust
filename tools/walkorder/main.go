// Prints blackfriday's Walk traversal order for a fixed tree, so the Rust
// port's walker can be checked against measured Go behaviour rather than a
// hand-derived expectation.
package main

import (
	"fmt"

	bf "github.com/russross/blackfriday/v2"
)

// doc -> [p1 -> t1, p2 -> t2]
func build() (*bf.Node, *bf.Node, *bf.Node) {
	doc := bf.NewNode(bf.Document)
	p1 := bf.NewNode(bf.Paragraph)
	t1 := bf.NewNode(bf.Text)
	p2 := bf.NewNode(bf.Paragraph)
	t2 := bf.NewNode(bf.Text)
	doc.AppendChild(p1)
	p1.AppendChild(t1)
	doc.AppendChild(p2)
	p2.AppendChild(t2)
	return doc, p1, p2
}

func main() {
	fmt.Println("## plain walk")
	doc, _, _ := build()
	doc.Walk(func(n *bf.Node, entering bool) bf.WalkStatus {
		fmt.Printf("%s %v\n", n.Type, entering)
		return bf.GoToNext
	})

	fmt.Println("## lone leaf")
	leaf := bf.NewNode(bf.Text)
	n := 0
	leaf.Walk(func(_ *bf.Node, _ bool) bf.WalkStatus { n++; return bf.GoToNext })
	fmt.Printf("visits=%d\n", n)

	fmt.Println("## skip children on entering Paragraph")
	doc2, _, _ := build()
	doc2.Walk(func(nd *bf.Node, entering bool) bf.WalkStatus {
		fmt.Printf("%s %v\n", nd.Type, entering)
		if nd.Type == bf.Paragraph && entering {
			return bf.SkipChildren
		}
		return bf.GoToNext
	})

	fmt.Println("## terminate immediately")
	doc3, _, _ := build()
	c := 0
	doc3.Walk(func(_ *bf.Node, _ bool) bf.WalkStatus { c++; return bf.Terminate })
	fmt.Printf("visits=%d\n", c)

	fmt.Println("## mutation mid-walk (append Emph to first Paragraph)")
	doc4, _, _ := build()
	appended := false
	doc4.Walk(func(nd *bf.Node, entering bool) bf.WalkStatus {
		fmt.Printf("%s %v\n", nd.Type, entering)
		if entering && nd.Type == bf.Paragraph && !appended {
			nd.AppendChild(bf.NewNode(bf.Emph))
			appended = true
		}
		return bf.GoToNext
	})

	fmt.Println("## unlink first/last child")
	doc5, p1, p2 := build()
	p3 := bf.NewNode(bf.Paragraph)
	doc5.AppendChild(p3)
	p2.Unlink()
	fmt.Printf("after unlink p2: first=%v last=%v p1.Next=%v p3.Prev=%v\n",
		doc5.FirstChild == p1, doc5.LastChild == p3, p1.Next == p3, p3.Prev == p1)
	fmt.Printf("p2 detached: parent=%v next=%v prev=%v\n", p2.Parent == nil, p2.Next == nil, p2.Prev == nil)

	fmt.Println("## String() truncation")
	s := bf.NewNode(bf.Text)
	s.Literal = []byte("short")
	fmt.Printf("%q\n", s.String())
	s.Literal = []byte("0123456789abcdefGHIJ")
	fmt.Printf("%q\n", s.String())
	s.Literal = []byte("0123456789abcdef")
	fmt.Printf("%q\n", s.String())
}

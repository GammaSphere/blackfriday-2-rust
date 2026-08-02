module github.com/GammaSphere/blackfriday-2-rust/fuzz

go 1.16

require (
	github.com/GammaSphere/blackfriday-2-rust/adapter v0.0.0
	github.com/russross/blackfriday/v2 v2.1.0
)

replace github.com/GammaSphere/blackfriday-2-rust/adapter => ../adapter

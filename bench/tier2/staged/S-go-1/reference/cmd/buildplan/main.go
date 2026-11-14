package main

import (
	"os"

	"example.com/buildplan/cli"
)

func main() {
	os.Exit(cli.Run(os.Args[1:], os.Stdout, os.Stderr))
}

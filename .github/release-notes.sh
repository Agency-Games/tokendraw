#!/bin/sh
# Prints the release notes from stdin with the install commands of the
# README before dist's download section:
#
#   .github/release-notes.sh VERSION <notes
set -eu
awk -v version="$1" '
/^## Download tokendraw / {
    print "## Install tokendraw " version
    print ""
    print "```sh"
    print "npm install -g tokendraw@" version
    print "cargo install --locked tokendraw --version " version
    print "```"
    print ""
    found = 1
}
{ print }
END {
    if (!found) {
        print "release-notes.sh: the notes have no download section" > "/dev/stderr"
        exit 1
    }
}
'

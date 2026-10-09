#!/bin/sh
# Prints the release notes from stdin with dist's npm section, which
# installs into the current project, replaced by the install commands of
# the README:
#
#   .github/release-notes.sh VERSION <notes
set -eu
awk -v version="$1" '
/^### Install prebuilt binaries into your npm project$/ {
    print "```sh"
    print "npm install -g tokendraw@" version
    print "cargo install --locked tokendraw --version " version
    print "```"
    skip = 1
    found = 1
    next
}
skip && /^```$/ { skip = 0; next }
!skip
END {
    if (!found) {
        print "release-notes.sh: the notes have no npm install section" > "/dev/stderr"
        exit 1
    }
}
'

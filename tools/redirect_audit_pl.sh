#!/bin/sh
# External payload for the redirect audit: writes O to fd1, E to fd2.
printf 'O\n'
printf 'E\n' >&2

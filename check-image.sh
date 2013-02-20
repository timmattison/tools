#!/usr/bin/env bash

# check-image.sh by Tim Mattison (tim@mattison.org)
# Version 0.1 - 2013-02-20

# Release history:
#   Version 0.1 - 2013-02-20 - Initial release

# This script was written to provide a fast way to determine if an image has been corrupted.
#   I wrote this when a certain "Cloud storage" provider trashed some of my files so I could
#   identify them quickly and restore them from backups.
#
# You do have backups, don't you?
#
# This script will print 0 if the file doesn't appear to be corrupt or 1 if it does appear
#   to be corrupt.  It has only been tested with JPEGs.

which identify &> /dev/null || { 
  echo "Couldn't find identify (from ImageMagick) on your PATH.  Install ImageMagick and try again."
  exit 1
}

if [ ! -e "$1" ];
then
  echo "Couldn't find the image specified or no image was specified"
  exit 2
fi

identify -verbose "$1" |& grep Corrupt &> /dev/null
echo $?

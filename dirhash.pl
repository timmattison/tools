#!/usr/bin/perl -w

use Data::Dumper;
# dirhash.pl by Tim Mattison (tim@mattison.org)
# Version 0.1 - 2012-01-29

# Release history:
#   Version 0.1 - 2012-01-29 - Initial release

# This script was written to provide a way to hash an entire directory tree.  This can
#   be handy when you have a directory where you need to verify that the contents and
#   filenames are identical but you may not want to touch the data using a program like
#   rsync.
#
# This was designed as a fast way to get a yes or no answer as to whether the contents
#   two directories are identical NOT as a way to synchronize them.  If one filename is
#   different, one byte inside any file is different, or there are missing/additional
#   files the output will be different.
#
# To test it try doing the following test.  Assume you have a directory called "data"
#   copy that directory to "data-copy" with "cp -R data data-copy".  Run dirhash.pl
#   on both directories and verify that you get the same hash.  Then modify a file,
#   a filename, add or delete a file from either and re-run dirhash.pl on them both.
#   The unmodified directory will have the same hash, the modified one will not.
#
# This script requires the Digest::SHA module.

# And here comes the real code...

# Check to see if we have the Digest::SHA module
eval {
  require Digest::SHA;
  Digest::SHA->import();
};

if($@) {
 die "Digest::SHA must be installed";
}

# Do we have any arguments?
if(@ARGV > 0) {
  # Hash each directory separately
  foreach my $source (@ARGV) {
    print "Source: $source " . dirhash($source) . "\n";
  }
}
else {
  # Not enough options, show the program usage information
  show_usage();
}

sub dirhash {
  my $source = $_[0];
  my $base_path = $_[1];

  # Is the source a directory?
  if(-d $source) {
    # Yes, process it recursively
    opendir SOURCE_DIRECTORY, $source;

    my @source_list;

    # Build a list of the entries in this directory
    while(readdir SOURCE_DIRECTORY) {
      my $directory_entry = $_;

      # Is this entry safe?
      if(($directory_entry ne ".") && ($directory_entry ne "..")) {
        # Yes, add it to the list
        push(@source_list, $source . "/" . $_);
      }
    }

    # Sort the list
    @source_list = sort(@source_list);

    # Create a destination for our master result
    my $result = "";

    # Hash each element individually
    foreach my $inner_source (@source_list) {
      $result .= dirhash($inner_source, $source);
    }

    # Hash the result itself
    $result = hash_string($result);

    # Return so we don't run this code on a raw directory
    return $result;
  }

  # This is a file, not a directory so the input file is the source
  my $input_file = $source;

  return hash_file($input_file, $base_path);
}

sub hash_string {
  my $input_string = $_[0];

  return hash($input_string, 0);
}

sub hash_file {
  my $input_file = $_[0];
  my $base_path = $_[1];

  return hash($input_file, 1, $base_path);
}

sub hash {
  my $input_data = $_[0];
  my $is_file = $_[1];
  my $base_path = $_[2];

  # Create a new instance of the SHA-512 algorithm object
  my $sha = Digest::SHA->new("SHA-512");

  # Is this a file?
  if($is_file == 1) { 
    # Yes, add the specified file to it
    $sha->addfile($input_data);

    my $hash_file = $input_data;

    # Is there a base path?
    if(defined($base_path)) {
      # Yes, remove it so that we get consistent results when the files are
      #   in different paths
      $hash_file = substr($input_data, length($base_path));
    }

    # Add the filename onto the end of the data to be hashed
    $sha->add(" " . $hash_file);
  }
  else {
    # No, hash the data as a string
    $sha->add($input_data);
  }

  # Return the base 64 digest
  return $sha->b64digest;
}

sub show_usage {
  print "Usage: PROGRAM DIRECTORY_1 DIRECTORY_2 ...\n";
  print "\n";
  exit;
}

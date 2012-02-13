#!/usr/bin/perl -w

use Data::Dumper;
# hostshash.pl by Tim Mattison (tim@mattison.org)
# Version 0.1 - 2012-02-13

# Release history:
#   Version 0.1 - 2012-02-13 - Initial release

# This script was written to provide a way to compare two hosts files to see if they
#   contain the same logical contents, not necessarily the same bytes.  This means
#   That if two hosts files hash to the same value with this script you can be sure
#   of the following:
#
#   - Each IP maps to the same number of hostnames
#   - Each IP maps to the same values of hostnames (case insensitive)
#
#   You CANNOT be sure of the following:
#
#   - The formatting is the same (all whitespace is treated the same)
#   - The comments are the same (they are completely ignored)

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
  # Hash each file separately
  foreach my $source (@ARGV) {
    print "Source: $source " . hostshash($source) . "\n";
  }
}
else {
  # Not enough options, show the program usage information
  show_usage();
}

sub hostshash {
  my $source = $_[0];

  # Is the source a directory?
  if(-d $source) {
    die "$source is a directory.  hostshash.pl can only hash host files.";
  }

  # This is a file, not a directory so the input file is the source
  my $input_file = $source;

  # Hash the file
  return hash($input_file);
}

sub hash {
  my $input_file = $_[0];

  # Does this a file exist?
  if(!-f $input_file) {
    # No, give up
    die "$input_file does not exist.";
  }

  # Create a new instance of the SHA-512 algorithm object
  my $sha = Digest::SHA->new("SHA-512");

  # Create a place to store our lines
  my @lines = ();

  # Read the data from the input file
  open INPUT_FILE, $input_file;

  while(<INPUT_FILE>) {
    my $line = $_;
    chomp $line;

    # Remove comments from the line
    $line =~ s/#.*$//g;

    # Replace all whitespace with a single space
    $line =~ s/\s+/ /g;

    # If the line is entirely whitespace or blank undef if
    if($line =~ m/^\s*$/) {
      undef $line;
    }

    # Is this line defined?
    if(defined($line)) {
      # Yes, add it to the array of lines
      push(@lines, $line);
    }
  }

  # Create the hash table for our IP entries
  my %ip_entries = ();

  for my $line (@lines) {
    my ($ip, @hostnames) = split(/ /, $line);

    if(!defined($ip_entries{$ip})) {
      $ip_entries{$ip} = ();
    }

    for my $hostname (@hostnames) {
      push(@{$ip_entries{$ip}}, $hostname);
    }
  }

  # Clear the array of lines
  @lines = ();

  # Loop through all entries in the hash
  for my $ip (keys %ip_entries) {
    my $line = $ip;

    # Make sure the hostnames are sorted
    my @hostnames = @{$ip_entries{$ip}};
    @hostnames = sort(@hostnames);

    # Copy all of the sorted hostnames into this record
    for my $hostname (@hostnames) {
      $line = $line . " $hostname";
    }

    $line = $line . "\n";

    # Add this record onto the lines array
    push(@lines, $line);
  }

  # Sort the lines
  my @sorted_lines = sort(@lines);

  # Join the lines in a single string
  my $joined_lines = join("\n", @sorted_lines);

  # Hash the joined lines
  $sha->add($joined_lines);

  # Return the base 64 digest
  return $sha->b64digest;
}

sub show_usage {
  print "Usage: PROGRAM HOST_FILE_1 HOST_FILE_2 ...\n";
  print "\n";
  exit;
}

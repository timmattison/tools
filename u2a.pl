#!/usr/bin/perl -w

# u2a.pl by Tim Mattison (tim@mattison.org)
# Version 0.11 - 2012-02-29

# Release history:
#   Version 0.1  - 2012-02-29 - Initial release

# This script was written to make it easier for scripts to be Unicode agnostic when dealing
#   with ASCII input files that are a mix of data generated from conventional scripts and
#   Hadoop code that may or may not have been encoded with Unicode 16.

# And here comes the real code...

# Make sure we have the Text::Iconv module
eval {
  require Text::Iconv;
  Text::Iconv->import("convert");
};

if($@) {
  die "The Text::Iconv module is required but this system does not appear to have it";
}

# Instantiate the text converter from UTF-16BE to ASCII
my $converter = Text::Iconv->new("UTF-16BE", "ASCII");

# Loop through all of the input text
while(<STDIN>) {
  my $line = $_;

  my $converted = $converter->convert($line);

  # Did the text convert?
  if(!defined($converted)) {
    # No, just use the input text.  It probably is ASCII already.
    $converted = $line;
  }

  # Output the converted text
  print $converted;
}

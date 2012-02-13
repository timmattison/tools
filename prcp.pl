#!/usr/bin/perl -w

# prcp.pl by Tim Mattison (tim@mattison.org)
# Version 0.11 - 2012-01-29

# Release history:
#   Version 0.1  - 2012-01-29 - Single file copy supported
#   Version 0.11 - 2012-01-29 - Multiple file copy to a directory supported

# This script was written to provide a copy command with some form of progress indicator.
#   I wrote it while using what appeared to be a very slow flash drive so I could make
#   sure that the data was actually copying.  Using the standard Linux "cp" program and
#   copying data to a vfat formatted flash drive doesn't provide any indication of what
#   is happening.  To make matters worse the "cp" program will allocate space on the
#   destination file system for the whole file ahead of time so you cannot see the file
#   size increase as data is copying.
#
# The decision to allocate space for the file makes sense since it guarantees that a
#   disk has adequate space for the file being copied but makes the entire copy process
#   too opaque with slow drives.
#
# This script only requires the File::Sync module to copy files but will display a
#   nicely formatted progress bar with a time indicator if you also have the
#   Term::ProgressBar module installed.  Other modules that make the experience nicer
#   are:
#
#  - File::Basename - Shortens the file name in the progress bar so it doesn't
#                       contain the whole source path
#
# If any of these modules aren't installed the functionality they provide is disabled
#   but the script will still function.

# Start with some constants...

# Read and write 1 MB at a time
my $BUFFER_MAX_SIZE = 1024 * 1024;

# And here comes the real code...

# Make sure we have the File::Sync module.  We need it so we don't get fooled by
#   disk caching and end up with a "bursty" progress readout.
eval {
  require File::Sync;
  File::Sync->import("fsync");
};

if($@) {
  die "The File::Sync module is required but this system does not appear to have it";
}

# Process the command line options.  I didn't like how getopt worked and my needs were
#   simple so I just did it myself.  Getopt::Std's getopt seems to deal with these
#   cases strangely if I use 'getopt("gv", \%arguments);":
#
# BAD #1:
# ./prcp.pl -g input.file output.file
#   - Puts "g" into argument hash
#   - Puts "input.file" into argument hash
#   - Leaves "output.file" in ARGV
#
# BAD #2:
# ./prcp.pl -v input.file output.file
#   - Puts "v" into argument hash
#   - Puts "input.file" into argument hash
#   - Leaves "output.file" in ARGV
#
# GOOD:
# ./prcp.pl -gv input.file output.file
#   - Puts "g" into argument hash
#   - Puts "v" into argument hash
#   - Leaves "input.file" in ARGV
#   - Leaves "output.file" in ARGV
#
# It may be something I'm doing wrong but it just isn't worth using another module if
#   it doesn't work like I expect it to.

# Assume they don't want these options until they specify them
my $guarantee = 0;
my $verify = 0;

# Loop through anything that looks like a switch
while(@ARGV && ($ARGV[0] =~ m/^-/)) {
  # Found a potential switch
  # Extract all of the characters after the dash
  my @characters = split //, substr($ARGV[0], 1);

  foreach my $character (@characters) {
    if($character eq "g") {
      # Guarantee that the file will fit on the destination drive
      print "Space guarantee enabled [NOT IMPLEMENTED YET]\n";
      $guarantee = 1;
    }
    elsif($character eq "v") {
      # Verify the file after copying
      print "Verification enabled [NOT IMPLEMENTED YET]\n";
      $verify = 1;
    }
    else {
      # Unknown, stop immediately
      show_usage();
    }
  }

  # Remove this argument from ARGV, it was processed
  shift @ARGV;
}

# Get the input and output files

# Do we have more than two arguments?
if(@ARGV == 2) {
  # No, just two arguments.  Do a normal file copy.
  progress_copy($ARGV[0], $ARGV[1]);
}
elsif(@ARGV > 2) {
  # Yes, all of the items except the last are files.  The last item must be a directory.
  my $output_directory = $ARGV[@ARGV - 1];

  # Does the output directory exist?
  if(!-d $output_directory) {
    # It either doesn't exist or isn't a directory
    die "$output_directory does not exist or is not a directory";
  }

  # Remove the output directory from the argument list
  undef $ARGV[@ARGV - 1];

  # Copy all of the files
  foreach my $input_file (@ARGV) {
    # Is this value defined?
    if(defined($input_file)) {
      # Yes, copy it
      progress_copy($input_file, $output_directory);
    }
    else {
      # No, this is probably the destination directory that we undef'd above.  Do nothing.
    }
  }
}
else {
  # Not enough options, show the program usage information
  show_usage();
}

sub progress_copy {
  my $source = $_[0];
  my $destination = $_[1];

  # Is the source a directory?
  if(-d $source) {
    # Yes, process it recursively
    opendir SOURCE_DIRECTORY, $source;

    while(readdir SOURCE_DIRECTORY) {
      my $directory_entry = $_;

      # Is this entry safe?
      if(($directory_entry ne ".") && ($directory_entry ne "..")) {
        # Yes, copy it
        progress_copy($source . "/" . $directory_entry, $destination);
      }
    }

    # Return so we don't run this code on a raw directory
    return;
  }

  # This is a file, not a directory so the input file is the source
  my $input_file = $source;

  # Prime the output file path
  my $output_file = $destination;

  # Prime the display name since they may not have the basename module
  my $display_name = $input_file;

  my $basename_supported = 0;

  # Check to see if we have the basename module
  eval {
    require File::Basename;
    File::Basename->import("basename");

    # If we got here we have the module and can determine the file's base name to keep
    #   the progress bar clean
    $display_name = basename($input_file);

    # Mark that this module is supported
    $basename_supported = 1;
  };

  # Is the destination a directory?
  if(-d $destination) {
    # Yes, is basename supported?
    if($basename_supported != 1) {
      # No, can't do directory copies without basename
      die "Directories as destinations are not supported without the File::Basename module";
    }
    else {
      # Yes, change the output file path
      $output_file = $destination . "/" . $display_name;

      # Remove any double forward slashes just to be clean
      $output_file =~ s/\/\//\//g;
    }
  }

  # Get the input file's size
  my $filesize = -s $input_file or die "Couldn't get the size of the input file [$input_file]";

  # Check to see if we have the progress bar module
  my $progress_bar;

  eval {
    require Term::ProgressBar;
    Term::ProgressBar->import();

    # If we got here the progress bar module is installed.  Instantiate a progress
    #   bar that measures from 0 to our input file's size so we can use it later.
    $progress_bar = Term::ProgressBar->new({ name => $display_name,
                                             count => $filesize,
                                             ETA => 'linear' });
  };

  open INPUT_FILE, "<$input_file" or die "Couldn't open the input file for reading";
  open OUTPUT_FILE, ">$output_file" or die "Couldn't open the output file for writing";

  binmode INPUT_FILE or die "Couldn't go to binary mode on the input file";
  binmode OUTPUT_FILE or die "Couldn't go to binary mode on the output file";

  my $offset = 0;

  my $bytes_read = sysread INPUT_FILE, $buffer, $BUFFER_MAX_SIZE;
  check_failure($bytes_read);

  while($bytes_read != 0) {
    my $bytes_written = syswrite OUTPUT_FILE, $buffer, length($buffer);
    fsync(\*OUTPUT_FILE) or die "fsync: $!";

    if($bytes_written != $bytes_read) {
      die "Bytes written does not equal bytes read, aborting";
    }

    $offset += length($buffer);

    # Do we have a progress bar?
    if(defined($progress_bar)) {
      # Yes, update it
      $progress_bar->update($offset);
    }

    #print "Offset: $offset\n";

    $bytes_read = sysread INPUT_FILE, $buffer, $BUFFER_MAX_SIZE;
    check_failure($bytes_read);
  }

  close INPUT_FILE;
  close OUTPUT_FILE;
}

sub check_failure {
  my $bytes_processed = $_[0];

  if(!defined($bytes_processed)) {
    die "No bytes processed: $!";
  }
}

sub show_usage {
  print "Usage: PROGRAM [-gv] INPUT_FILE OUTPUT_FILE\n";
  print "\n";
  print "  -g - Guarantee that the file will fit on the destination file system before copying\n";
  print "  -v - Verify that the destination matches the source after copying using a hash\n";
  print "\n";
  print "  NOTE: File verification will increase the amount of time required to copy files!\n";
  print "\n";
  exit;
}

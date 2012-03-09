#!/usr/bin/perl -w

# prcp.pl by Tim Mattison (tim@mattison.org)
# Version 0.11 - 2012-01-29

# Release history:
#   Version 0.1  - 2012-01-29 - Single file copy supported
#   Version 0.11 - 2012-01-29 - Multiple file copy to a directory supported
#   Version 0.2  - 2012-03-01 - Added a carriage return after each file is
#                                 copied so the users sees all of their statuses
#                                 not just the last one
#   Version 0.3  - 2012-03-01 - Implemented overall progress option (-o)
my $version = "0.3";
my $release_date = "2012-03-01";

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
my $overall_progress = 0;
my $overall_size = 0;
my $overall_offset = 0;
my $overall_progress_bar;

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
    elsif($character eq "o") {
      # Show overall progress instead of per file progress
      $overall_progress = 1;
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
my $input = $ARGV[0];
my $output = $ARGV[1];

# Are we doing an overall progress indicator?
if($overall_progress == 1) {
  # Yes, calculate the overall size
  $overall_size = calculate_overall_size($input);
  $overall_progress_bar = create_progress_bar($input . " -> " . $output, $overall_size);
}

# Do we have more than two arguments?
if(@ARGV == 2) {
  # No, just two arguments.  Do a normal file copy.
  progress_copy($input, $output);
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
    # Yes, does the destination exist?
    if(-e $destination) {
      # Yes, is it a directory?
      if(!-d $destination) {
        # No, this can't be done
        die "The source is a directory [$source] but the destination [$destination] is not.  Cannot continue.";
      }
      else {
        # Yes, source and destination are both directories
      }
    }
    else {
      # No, the destination does not exist.  Create it.
      mkdir $destination;

      # Are we doing an overall progress indicator?
      if($overall_progress == 0) {
        # No, let the user know that we're creating a directory
        print "Directory $destination created\n";
      }
      else {
        # Yes, show nothing so we don't break the progress indicator
      }
    }

    # Everything looks good.  Copy the source recursively.
    my $SOURCE_DIRECTORY;
    opendir $SOURCE_DIRECTORY, $source;

    while(readdir $SOURCE_DIRECTORY) {
      my $directory_entry = $_;
      my $new_source = $source . "/" . $directory_entry;
      my $new_destination = $destination . "/" . $directory_entry;

      # Is this entry safe?
      if(is_directory_safe_to_traverse($directory_entry)) {
        # Yes, copy it
        progress_copy($new_source, $new_destination);
      }
    }

    closedir $SOURCE_DIRECTORY;

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

  my $progress_bar;
  my $offset = 0;

  # Do we need a new progress bar?
  if($overall_progress == 0) {
    # Yes, we are not doing overall progress we are doing per file progress
    $progress_bar = create_progress_bar($display_name, $filesize);
  }
  else {
    # No, reuse our overall progress bar
    $progress_bar = $overall_progress_bar;

    # Set our offset to the overall offset
    $offset = $overall_offset;
  }

  open INPUT_FILE, "<$input_file" or die "Couldn't open the input file for reading";
  open OUTPUT_FILE, ">$output_file" or die "Couldn't open the output file for writing";

  binmode INPUT_FILE or die "Couldn't go to binary mode on the input file";
  binmode OUTPUT_FILE or die "Couldn't go to binary mode on the output file";

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

    $bytes_read = sysread INPUT_FILE, $buffer, $BUFFER_MAX_SIZE;
    check_failure($bytes_read);
  }

  close INPUT_FILE;
  close OUTPUT_FILE;

  # Are we doing an overall progress bar?
  if($overall_progress == 0) {
    # No, move to the next line so that the user can see the status of all of their files
    #   individually
    print "\n";
  }
  else {
    # Yes, we are doing an overall progress bar.  Do not insert any carriage returns so
    #   the overall status stays on the same line.

    # Update the overall offset
    $overall_offset = $offset;
  }
}

sub calculate_overall_size {
  my $source = $_[0];
  my $size = 0;

  # Is the source a directory?
  if(-d $source) {
    # Everything looks good.  Copy the source recursively.
    my $SOURCE_DIRECTORY;
    opendir $SOURCE_DIRECTORY, $source;

    while(readdir $SOURCE_DIRECTORY) {
      my $directory_entry = $_;
      my $new_source = $source . "/" . $directory_entry;

      # Is this a file?
      if(-f $new_source) {
        # Yes, just add its size to the total
        $size += -s $new_source;
      }
      else {
        # No, is this a directory?
        if(-d $new_source) {
          # Yes, is this directory safe?
          if(is_directory_safe_to_traverse($directory_entry)) {
            # Yes, add its size to the total size
            $size += calculate_overall_size($new_source);
          }
        }
      }
    }

    closedir $SOURCE_DIRECTORY;
  }

  # Return the total size
  return $size;
}

sub is_directory_safe_to_traverse {
  my $directory_entry = $_[0];

  # Is this directory safe to traverse?
  if(($directory_entry ne ".") && ($directory_entry ne "..")) {
    # Yes, it is not "." (the current directory which would cause an infinite loop) and it
    #   is not ".." (which would cause another infinite loop by going up the file system
    #   to the parent directory)
    return 1;
  }
  else {
    # No, this is not safe
    return 0;
  }
}

sub check_failure {
  my $bytes_processed = $_[0];

  if(!defined($bytes_processed)) {
    die "No bytes processed: $!";
  }
}

sub show_usage {
  print "PRogress CoPy (prcp.pl) - v$version - $release_date\n";
  print "Usage: PROGRAM [-gv] INPUT_FILE OUTPUT_FILE\n";
  print "\n";
  print "  -o - Show the overall progress instead of a per file progress indicator\n";
  print "  -g - Guarantee that the file will fit on the destination file system before copying\n";
  print "  -v - Verify that the destination matches the source after copying using a hash\n";
  print "\n";
  print "  NOTE: File verification will increase the amount of time required to copy files!\n";
  print "\n";
  exit;
}

sub create_progress_bar {
  my $display_name = $_[0];
  my $filesize = $_[1];
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

  return $progress_bar;
}

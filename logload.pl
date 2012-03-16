#!/usr/bin/perl -w

use Data::Dumper;

# logload.pl by Tim Mattison (tim@mattison.org)
# Version 0.1 - 2012-03-16

# Release history:
#   Version 0.1  - 2012-03-16 - First release
my $version = "0.1";
my $release_date = "2012-03-16";

# This script was written to log system performance characteristics in the simplest way
#   possible.  The motivation for this was to document and debug poor performance that
#   I was seeing sporadically on different virtual private server services.
#
# This is dead simple.  There is no log rolling, no fancy charts, nothing.  This is just
#   intended to give you the raw data so you can process it on your own.  In the future
#   I may add on some features to make this easier.

# The file to which we're going to write our output
my $OUTPUT_FILE = "cpu-load.log";

# How long we should wait and gather statistics
my $SLEEP_TIME = 5;

# And here comes the real code...

# Make sure we have the Sys::Statistics::Linux::CpuStats module
eval {
  require Sys::Statistics::Linux::CpuStats;
};

if($@) {
  die "The Sys::Statistics::Linux::CpuStats module is required but this system does not appear to have it";
}

# Instantiate the stat_grabber object
my $stat_grabber = Sys::Statistics::Linux::CpuStats->new;

# Initialize the stats
$stat_grabber->init;

# Sleep for a few seconds so the stats aren't blank (the documentation indicates you
#   must do this)
sleep $SLEEP_TIME;

# Get the stats
my $stats = $stat_grabber->get;

# Does the output file exist?
if(!-e $OUTPUT_FILE) {
  # No, create it and print the header info
  open OUTPUT_FILE, ">$OUTPUT_FILE";
  print OUTPUT_FILE "CPU Number,Reading Time,System,SoftIRQ,Idle,Nice,IRQ,Steal,User,IOWait,Total\n";
}
else {
  # Open the output file for appending
  open OUTPUT_FILE, ">>$OUTPUT_FILE";
}

# Loop through the CPUs
my $cpu_number = 0;

# Get the current epoch time
my $epoch = time;

# Get the current CPU's data
my $current_record = $stats->{"cpu" . $cpu_number};

while(defined($current_record)) {
  # Get the current record

  # Print out the fields in the order we expect
  print OUTPUT_FILE $cpu_number;
  print OUTPUT_FILE ",";
  print OUTPUT_FILE $epoch;
  print OUTPUT_FILE ",";
  print OUTPUT_FILE $current_record->{system};
  print OUTPUT_FILE ",";
  print OUTPUT_FILE $current_record->{softirq};
  print OUTPUT_FILE ",";
  print OUTPUT_FILE $current_record->{idle};
  print OUTPUT_FILE ",";
  print OUTPUT_FILE $current_record->{nice};
  print OUTPUT_FILE ",";
  print OUTPUT_FILE $current_record->{irq};
  print OUTPUT_FILE ",";
  print OUTPUT_FILE $current_record->{steal};
  print OUTPUT_FILE ",";
  print OUTPUT_FILE $current_record->{user};
  print OUTPUT_FILE ",";
  print OUTPUT_FILE $current_record->{iowait};
  print OUTPUT_FILE ",";
  print OUTPUT_FILE $current_record->{total};
  print OUTPUT_FILE "\n";

  # Move onto the next CPU
  $cpu_number++;
  $current_record = $stats->{"cpu" . $cpu_number};
}

# Close the file
close OUTPUT_FILE;

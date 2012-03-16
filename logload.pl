#!/usr/bin/perl -w

use Data::Dumper;

# logload.pl by Tim Mattison (tim@mattison.org)
# Version 0.1 - 2012-03-16

# Release history:
#   Version 0.1  - 2012-03-16 - First release
#   Version 0.2  - 2012-03-16 - Added secondary load average log file
my $version = "0.2";
my $release_date = "2012-03-16";

# This script was written to log system performance characteristics in the simplest way
#   possible.  The motivation for this was to document and debug poor performance that
#   I was seeing sporadically on different virtual private server services.
#
# This is dead simple.  There is no log rolling, no fancy charts, nothing.  This is just
#   intended to give you the raw data so you can process it on your own.  In the future
#   I may add on some features to make this easier.

# The files to which we're going to write our output
my $CPU_OUTPUT_FILE = "cpu-load.log";
my $LOAD_AVERAGE_OUTPUT_FILE = "load-average.log";

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

# Does the CPU output file exist?
if(!-e $CPU_OUTPUT_FILE) {
  # No, create it and print the header info
  open CPU_OUTPUT_FILE, ">$CPU_OUTPUT_FILE";
  print CPU_OUTPUT_FILE "CPU Number,Reading Time,System,SoftIRQ,Idle,Nice,IRQ,Steal,User,IOWait,Total\n";
}
else {
  # Open the CPU output file for appending
  open CPU_OUTPUT_FILE, ">>$CPU_OUTPUT_FILE";
}

# Does the load average output file exist?
if(!-e $LOAD_AVERAGE_OUTPUT_FILE) {
  # No, create it and print the header info
  open LOAD_AVERAGE_OUTPUT_FILE, ">$LOAD_AVERAGE_OUTPUT_FILE";
  print LOAD_AVERAGE_OUTPUT_FILE "Reading Time,5 minute,10 minute,15 minute,Number of processes,Most recent PID\n";
}
else {
  # Open the load average output file for appending
  open LOAD_AVERAGE_OUTPUT_FILE, ">>$LOAD_AVERAGE_OUTPUT_FILE";
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
  print CPU_OUTPUT_FILE $cpu_number;
  print CPU_OUTPUT_FILE ",";
  print CPU_OUTPUT_FILE $epoch;
  print CPU_OUTPUT_FILE ",";
  print CPU_OUTPUT_FILE $current_record->{system};
  print CPU_OUTPUT_FILE ",";
  print CPU_OUTPUT_FILE $current_record->{softirq};
  print CPU_OUTPUT_FILE ",";
  print CPU_OUTPUT_FILE $current_record->{idle};
  print CPU_OUTPUT_FILE ",";
  print CPU_OUTPUT_FILE $current_record->{nice};
  print CPU_OUTPUT_FILE ",";
  print CPU_OUTPUT_FILE $current_record->{irq};
  print CPU_OUTPUT_FILE ",";
  print CPU_OUTPUT_FILE $current_record->{steal};
  print CPU_OUTPUT_FILE ",";
  print CPU_OUTPUT_FILE $current_record->{user};
  print CPU_OUTPUT_FILE ",";
  print CPU_OUTPUT_FILE $current_record->{iowait};
  print CPU_OUTPUT_FILE ",";
  print CPU_OUTPUT_FILE $current_record->{total};
  print CPU_OUTPUT_FILE "\n";

  # Move onto the next CPU
  $cpu_number++;
  $current_record = $stats->{"cpu" . $cpu_number};
}

# Close the CPU output file
close CPU_OUTPUT_FILE;

# Get the load average
my $load_average = `cat /proc/loadavg`;

# Replace the spaces with commas
$load_average =~ s/ /,/g;

# Put it in our output file
print LOAD_AVERAGE_OUTPUT_FILE $load_average;

# Close the load average output file
close LOAD_AVERAGE_OUTPUT_FILE;

package main

import (
	"flag"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"text/tabwriter"

	"github.com/timmattison/tools/internal/version"
)

func main() {
	var showEnv = flag.Bool("env", false, "Show environment variables")
	var showFiles = flag.Bool("files", true, "Show open files")
	var showNetwork = flag.Bool("net", true, "Show network connections")
	var showAll = flag.Bool("all", false, "Show all information")
	var verbose = flag.Bool("v", false, "Verbose output")
	var caseSensitive = flag.Bool("case-sensitive", false, "Use case-sensitive matching for process name")
	var showVersion bool
	flag.BoolVar(&showVersion, "version", false, "Show version information")
	flag.BoolVar(&showVersion, "V", false, "Show version information (shorthand)")

	flag.Usage = func() {
		fmt.Fprintf(os.Stderr, "Usage: %s [options] <process-name>\n\n", os.Args[0])
		fmt.Fprintf(os.Stderr, "Find processes by name and display detailed information about them.\n\n")
		fmt.Fprintf(os.Stderr, "Options:\n")
		flag.PrintDefaults()
		fmt.Fprintf(os.Stderr, "\nExample: procinfo chrome\n")
	}

	flag.Parse()

	if showVersion {
		fmt.Println(version.String("procinfo"))
		os.Exit(0)
	}

	if flag.NArg() < 1 {
		flag.Usage()
		os.Exit(1)
	}

	// If --all is specified, enable all options
	if *showAll {
		*showEnv = true
		*showFiles = true
		*showNetwork = true
	}

	processName := flag.Arg(0)
	fmt.Printf("Searching for processes matching: %s\n\n", processName)

	// Find PIDs matching the process name
	pids, err := findProcesses(processName, *caseSensitive)
	if err != nil {
		fmt.Printf("Error finding processes: %v\n", err)
		os.Exit(1)
	}

	if len(pids) == 0 {
		fmt.Printf("No processes found matching: %s\n", processName)
		os.Exit(0)
	}

	fmt.Printf("Found %d matching processes\n\n", len(pids))

	// Display information for each process
	for _, pid := range pids {
		displayProcessInfo(pid, *showEnv, *showFiles, *showNetwork, *verbose)
	}
}

func findProcesses(name string, caseSensitive bool) ([]string, error) {
	var cmd *exec.Cmd

	if caseSensitive {
		// Use standard pgrep for case-sensitive search
		cmd = exec.Command("pgrep", "-f", name)
	} else {
		// Use pgrep with -i flag for case-insensitive search
		cmd = exec.Command("pgrep", "-i", "-f", name)
	}

	output, err := cmd.Output()
	if err != nil {
		// pgrep returns exit code 1 when no processes match
		if exitErr, ok := err.(*exec.ExitError); ok && exitErr.ExitCode() == 1 {
			return []string{}, nil
		}
		return nil, err
	}

	pids := strings.Split(strings.TrimSpace(string(output)), "\n")
	return pids, nil
}

func displayProcessInfo(pid string, showEnv, showFiles, showNetwork, verbose bool) {
	fmt.Printf("=== Process ID: %s ===\n", pid)

	// Get basic process info with wider command column
	// Use ww to get unlimited width for command column
	cmd := exec.Command("ps", "-p", pid, "-o", "user,pid,ppid,pcpu,pmem,start,time,comm=COMMAND", "-ww")
	output, err := cmd.Output()
	if err == nil {
		fmt.Println(strings.TrimSpace(string(output)))

		// Extract the user from ps output to check permissions
		lines := strings.Split(strings.TrimSpace(string(output)), "\n")
		if len(lines) > 1 {
			fields := strings.Fields(lines[1])
			if len(fields) > 0 {
				processUser := fields[0]
				currentUser := os.Getenv("USER")
				if processUser != currentUser && currentUser != "root" {
					fmt.Printf("\n⚠️  Warning: Process is owned by user '%s', but you're running as '%s'.\n",
						processUser, currentUser)
					fmt.Printf("   Some information may not be accessible. Try running with sudo for full details.\n\n")
				}
			}
		}
	} else {
		fmt.Printf("Error getting process info: %v\n", err)
	}

	// Get full command line
	cmdlineFile := fmt.Sprintf("/proc/%s/cmdline", pid)
	cmdlineBytes, err := os.ReadFile(cmdlineFile)
	if err == nil && len(cmdlineBytes) > 0 {
		// Replace null bytes with spaces for display
		cmdline := strings.ReplaceAll(string(cmdlineBytes), "\x00", " ")
		fmt.Println("\nFull Command Line:")
		fmt.Println(cmdline)
	} else if verbose {
		fmt.Printf("Error getting command line: %v\n", err)
	} else if err != nil && strings.Contains(err.Error(), "permission denied") {
		fmt.Println("\nFull Command Line: Permission denied - try running with sudo")
	}

	// Get working directory
	cmd = exec.Command("pwdx", pid)
	output, err = cmd.Output()
	if err == nil {
		fmt.Println("\nWorking Directory:")
		fmt.Println(strings.TrimSpace(string(output)))
	} else if verbose {
		fmt.Printf("Error getting working directory: %v\n", err)
	} else if strings.Contains(err.Error(), "permission denied") {
		fmt.Println("\nWorking Directory: Permission denied - try running with sudo")
	}

	// Get environment variables if requested
	if showEnv {
		cmd = exec.Command("cat", fmt.Sprintf("/proc/%s/environ", pid))
		output, err = cmd.Output()
		if err == nil {
			fmt.Println("\nEnvironment Variables:")
			envVars := strings.Split(string(output), "\x00")
			w := tabwriter.NewWriter(os.Stdout, 0, 0, 2, ' ', 0)
			for _, env := range envVars {
				if env != "" {
					parts := strings.SplitN(env, "=", 2)
					if len(parts) == 2 {
						fmt.Fprintf(w, "%s\t%s\n", parts[0], parts[1])
					} else {
						fmt.Fprintf(w, "%s\t\n", parts[0])
					}
				}
			}
			w.Flush()
		} else if verbose {
			fmt.Printf("Error getting environment variables: %v\n", err)
		} else if strings.Contains(err.Error(), "permission denied") {
			fmt.Println("\nEnvironment Variables: Permission denied - try running with sudo")
		}
	}

	// Get open files if requested
	if showFiles {
		cmd = exec.Command("lsof", "-p", pid)
		output, err = cmd.Output()
		if err == nil && len(output) > 0 {
			fmt.Println("\nOpen Files:")
			// Process lsof output to make it more readable
			lines := strings.Split(strings.TrimSpace(string(output)), "\n")
			if len(lines) > 1 {
				// Print header
				fmt.Println(lines[0])
				// Print files (skip header)
				for _, line := range lines[1:] {
					fmt.Println(line)
				}
			} else {
				fmt.Println("No open files found")
			}
		} else if verbose {
			fmt.Printf("Error getting open files: %v\n", err)
		} else if err != nil && strings.Contains(err.Error(), "permission denied") {
			fmt.Println("\nOpen Files: Permission denied - try running with sudo")
		}
	}

	// Get network connections if requested
	if showNetwork {
		cmd = exec.Command("ss", "-p", fmt.Sprintf("pid = %s", pid))
		output, err = cmd.Output()
		if err == nil && len(output) > 0 {
			fmt.Println("\nNetwork Connections:")
			fmt.Println(strings.TrimSpace(string(output)))
		} else if verbose {
			fmt.Printf("Error getting network connections: %v\n", err)
		} else if err != nil && strings.Contains(err.Error(), "permission denied") {
			fmt.Println("\nNetwork Connections: Permission denied - try running with sudo")
		}
	}

	fmt.Println()
}

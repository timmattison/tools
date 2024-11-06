package main

import (
	"fmt"
	"github.com/aws/aws-sdk-go/aws"
	"github.com/aws/aws-sdk-go/aws/credentials"
	"github.com/aws/aws-sdk-go/aws/session"
	"github.com/aws/aws-sdk-go/service/sts"
	"golang.design/x/clipboard"
	"os"
	"path/filepath"
	"strings"
)

const (
	UpperAwsAccessKeyId     = "AWS_ACCESS_KEY_ID"
	UpperAwsSecretAccessKey = "AWS_SECRET_ACCESS_KEY"
	UpperAwsSessionToken    = "AWS_SESSION_TOKEN"
)

func main() {
	err := clipboard.Init()

	if err != nil {
		panic(err)
	}

	clipboardData := string(clipboard.Read(clipboard.FmtText))

	clipboardStrings := strings.Split(clipboardData, "\n")

	var awsAccessKeyId string
	var awsSecretAccessKey string
	var awsSessionToken string

	if len(clipboardStrings) == 3 {
		awsAccessKeyId = stringContaining(clipboardStrings, UpperAwsAccessKeyId)
		awsSecretAccessKey = stringContaining(clipboardStrings, UpperAwsSecretAccessKey)
		awsSessionToken = stringContaining(clipboardStrings, UpperAwsSessionToken)
	} else if len(clipboardStrings) == 4 {
		awsAccessKeyId = stringContaining(clipboardStrings, strings.ToLower(UpperAwsAccessKeyId))
		awsSecretAccessKey = stringContaining(clipboardStrings, strings.ToLower(UpperAwsSecretAccessKey))
		awsSessionToken = stringContaining(clipboardStrings, strings.ToLower(UpperAwsSessionToken))
	} else {
		fmt.Printf("👎 Expected 3 or 4 lines in clipboard\n")
		os.Exit(1)
	}

	if awsAccessKeyId == "" || !strings.Contains(awsAccessKeyId, "=") {
		fmt.Printf("👎 Could not find the AWS access key ID in the clipboard\n")
		os.Exit(1)
	}

	if awsSecretAccessKey == "" || !strings.Contains(awsSecretAccessKey, "=") {
		fmt.Printf("👎 Could not find the AWS secret access key in the clipboard\n")
		os.Exit(1)
	}

	if awsSessionToken == "" || !strings.Contains(awsSessionToken, "=") {
		fmt.Printf("👎 Could not find the AWS session token in the clipboard\n")
		os.Exit(1)
	}

	if !strings.Contains(awsAccessKeyId, "=") {
		fmt.Printf("👎 Could not find the AWS access key ID assignment in the clipboard\n")
		os.Exit(1)
	}

	if !strings.Contains(awsSecretAccessKey, "=") {
		fmt.Printf("👎 Could not find the AWS secret access key assignment in the clipboard\n")
		os.Exit(1)
	}

	if !strings.Contains(awsSessionToken, "=") {
		fmt.Printf("👎 Could not find the AWS session token assignment in the clipboard\n")
		os.Exit(1)
	}

	awsAccessKeyId = strings.Split(awsAccessKeyId, "=")[1]
	awsAccessKeyId = strings.ReplaceAll(awsAccessKeyId, "\"", "")
	awsAccessKeyId = strings.TrimSpace(awsAccessKeyId)
	awsSecretAccessKey = strings.Split(awsSecretAccessKey, "=")[1]
	awsSecretAccessKey = strings.ReplaceAll(awsSecretAccessKey, "\"", "")
	awsSecretAccessKey = strings.TrimSpace(awsSecretAccessKey)
	awsSessionToken = strings.Split(awsSessionToken, "=")[1]
	awsSessionToken = strings.ReplaceAll(awsSessionToken, "\"", "")
	awsSessionToken = strings.TrimSpace(awsSessionToken)

	mySession := session.Must(session.NewSession())

	awsCredentials := credentials.NewStaticCredentials(awsAccessKeyId, awsSecretAccessKey, awsSessionToken)
	awsConfig := aws.NewConfig().WithCredentials(awsCredentials)

	stsClient := sts.New(mySession, awsConfig)

	var getCallerIdentityOutput *sts.GetCallerIdentityOutput
	getCallerIdentityOutput, err = stsClient.GetCallerIdentity(&sts.GetCallerIdentityInput{})

	if err != nil {
		fmt.Printf("👎 Credentials provided are not valid. Skipping update.\n")
		fmt.Printf("Error: %s\n", err)
		os.Exit(1)
	}

	var home string
	home, err = os.UserHomeDir()

	if err != nil {
		fmt.Printf("👎 Could not determine home directory\n")
		os.Exit(1)
	}

	var awsCredentialsPath string
	awsCredentialsPath = filepath.Join(home, ".aws/credentials")

	var awsCredentialsFile *os.File
	awsCredentialsFile, err = os.Create(awsCredentialsPath)
	defer awsCredentialsFile.Close()

	if err != nil {
		fmt.Printf("👎 Could not create AWS credentials file\n")
		os.Exit(1)
	}

	output := ""
	output += "[default]"
	output += "\n"
	output += fmt.Sprintf("aws_access_key_id = %s", awsAccessKeyId)
	output += "\n"
	output += fmt.Sprintf("aws_secret_access_key = %s", awsSecretAccessKey)
	output += "\n"
	output += fmt.Sprintf("aws_session_token = %s", awsSessionToken)
	output += "\n"

	_, err = awsCredentialsFile.WriteString(output)

	if err != nil {
		fmt.Printf("👎 Could not write to AWS credentials file\n")
		os.Exit(1)
	}

	fmt.Println("👍 Credentials updated successfully. Your AWS default profile is now set to the credentials in your clipboard. ")
	fmt.Println()
	fmt.Printf("Your AWS account ID is %s\n", *getCallerIdentityOutput.Account)
	fmt.Printf("Your AWS user ID is %s\n", *getCallerIdentityOutput.UserId)
}

func stringContaining(input []string, pattern string) string {
	for _, line := range input {
		if strings.Contains(line, pattern) {
			return line
		}
	}

	return ""
}

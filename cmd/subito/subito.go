package main

import (
	"context"
	"flag"
	"fmt"
	"os"

	"github.com/charmbracelet/log"
	mqtt "github.com/eclipse/paho.mqtt.golang"
	iot "github.com/timmattison/aws-iot-core-websockets-go"
	"github.com/timmattison/tools/internal/version"
)

func main() {
	var showVersion bool
	flag.BoolVar(&showVersion, "version", false, "Show version information")
	flag.BoolVar(&showVersion, "V", false, "Show version information (shorthand)")

	flag.Parse()

	if showVersion {
		fmt.Println(version.String("subito"))
		os.Exit(0)
	}

	topics := flag.Args()

	if len(topics) == 0 {
		log.Info("You must provide at least one AWS IoT topic to subscribe to")
		os.Exit(1)
	}

	ctx := context.Background()

	var mqttOptions *mqtt.ClientOptions
	var err error

	if mqttOptions, err = iot.NewMqttOptions(ctx, iot.Options{}); err != nil {
		log.Fatal("Could not create MQTT options", "error", err)
	}

	client := mqtt.NewClient(mqttOptions)

	token := client.Connect()

	if token.Wait() && token.Error() != nil {
		log.Fatal("Could not connect to AWS IoT", "error", token.Error())
		return
	}

	for _, topic := range topics {
		client.Subscribe(topic, 0, func(client mqtt.Client, message mqtt.Message) {
			log.Infof("\nTopic: %s\nMessage: %s", message.Topic(), message.Payload())
		})

		log.Info("Subscribed", "topic", topic)
	}

	select {}
}

//! The command line of `subito`.

use clap::Parser;
use rumqttc::QoS;

/// Subscribe to AWS IoT Core topics and print every message that arrives.
#[derive(Parser, Debug)]
#[command(name = "subito", version = buildinfo::version_string!())]
pub struct Cli {
    /// The topics to subscribe to. An MQTT wildcard is allowed.
    pub topics: Vec<String>,

    /// The quality of service of each subscription.
    #[arg(long)]
    pub qos: u8,

    /// The AWS IoT data endpoint. This skips the call to `DescribeEndpoint`.
    #[arg(long)]
    pub endpoint: Option<String>,

    /// Print a payload that holds JSON with indentation.
    #[arg(long)]
    pub json: bool,
}

impl Cli {
    /// Gives the quality of service as the MQTT client states it.
    #[must_use]
    pub fn mqtt_qos(&self) -> QoS {
        unimplemented!("the quality of service map")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name of the program, as the first word of a command line.
    const PROGRAM: &str = "subito";

    /// Parses a command line that must parse.
    ///
    /// `Parser::parse_from` stops the process when the command line is bad,
    /// which stops every other test in the same binary. This helper keeps the
    /// failure inside the one test that made it.
    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("this command line must parse")
    }

    #[test]
    fn two_topics_land_in_the_topics_field_in_order() {
        let cli = parse(&[PROGRAM, "first/topic", "second/topic"]);

        assert_eq!(cli.topics, vec!["first/topic", "second/topic"]);
    }

    #[test]
    fn a_single_level_wildcard_topic_parses_unchanged() {
        let cli = parse(&[PROGRAM, "sensors/+/temperature"]);

        assert_eq!(cli.topics, vec!["sensors/+/temperature"]);
    }

    #[test]
    fn a_multi_level_wildcard_topic_parses_unchanged() {
        let cli = parse(&[PROGRAM, "sensors/#"]);

        assert_eq!(cli.topics, vec!["sensors/#"]);
    }

    #[test]
    fn the_quality_of_service_is_zero_when_the_command_line_gives_none() {
        let cli = parse(&[PROGRAM, "sensors/#"]);

        assert_eq!(cli.qos, 0);
    }

    #[test]
    fn a_quality_of_service_of_one_parses() {
        let cli = parse(&[PROGRAM, "--qos", "1", "sensors/#"]);

        assert_eq!(cli.qos, 1);
    }

    #[test]
    fn a_quality_of_service_of_two_parses() {
        let cli = parse(&[PROGRAM, "--qos", "2", "sensors/#"]);

        assert_eq!(cli.qos, 2);
    }

    #[test]
    fn a_quality_of_service_of_three_is_refused() {
        let result = Cli::try_parse_from([PROGRAM, "--qos", "3", "sensors/#"]);

        assert!(result.is_err(), "a QoS of 3 must not parse");
    }

    #[test]
    fn a_negative_quality_of_service_is_refused() {
        let result = Cli::try_parse_from([PROGRAM, "--qos", "-1", "sensors/#"]);

        assert!(result.is_err(), "a QoS of -1 must not parse");
    }

    #[test]
    fn the_endpoint_flag_lands_in_the_endpoint_field() {
        let cli = parse(&[PROGRAM, "--endpoint", "host", "sensors/#"]);

        assert_eq!(cli.endpoint.as_deref(), Some("host"));
    }

    #[test]
    fn the_endpoint_is_absent_when_the_command_line_gives_none() {
        let cli = parse(&[PROGRAM, "sensors/#"]);

        assert_eq!(cli.endpoint, None);
    }

    #[test]
    fn json_is_false_when_the_command_line_gives_no_flag() {
        let cli = parse(&[PROGRAM, "sensors/#"]);

        assert!(!cli.json);
    }

    #[test]
    fn the_json_flag_sets_json() {
        let cli = parse(&[PROGRAM, "--json", "sensors/#"]);

        assert!(cli.json);
    }

    #[test]
    fn a_quality_of_service_of_zero_becomes_at_most_once() {
        let cli = parse(&[PROGRAM, "--qos", "0", "sensors/#"]);

        assert_eq!(cli.mqtt_qos(), QoS::AtMostOnce);
    }

    #[test]
    fn a_quality_of_service_of_one_becomes_at_least_once() {
        let cli = parse(&[PROGRAM, "--qos", "1", "sensors/#"]);

        assert_eq!(cli.mqtt_qos(), QoS::AtLeastOnce);
    }

    #[test]
    fn a_quality_of_service_of_two_becomes_exactly_once() {
        let cli = parse(&[PROGRAM, "--qos", "2", "sensors/#"]);

        assert_eq!(cli.mqtt_qos(), QoS::ExactlyOnce);
    }
}

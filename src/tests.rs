#[cfg(test)]
mod tests {
    use crate::cli::Cli;
    use crate::utils;
    use clap::Parser;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, LabelSelectorRequirement};

    #[test]
    fn test_cli_parsing_deployment() {
        let args = vec!["kubectl-tail", "deployment/my-deployment"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.resources, vec!["deployment/my-deployment".to_string()]);
        assert!(cli.selector.is_none());
    }

    #[test]
    fn test_cli_parsing_pod() {
        let args = vec!["kubectl-tail", "my-pod"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.resources, vec!["my-pod".to_string()]);
    }

    #[test]
    fn test_cli_parsing_labels() {
        let args = vec!["kubectl-tail", "-l", "app=nginx"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.selector, Some("app=nginx".to_string()));
    }

    #[test]
    fn test_cli_parsing_container() {
        let args = vec!["kubectl-tail", "my-pod", "-c", "app"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.container, Some("app".to_string()));
    }

    #[test]
    fn test_cli_parsing_multiple_resources() {
        let args = vec!["kubectl-tail", "deployment/app1", "pod/my-pod"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(
            cli.resources,
            vec!["deployment/app1".to_string(), "pod/my-pod".to_string()]
        );
    }

    #[test]
    fn test_cli_parsing_tail() {
        let args = vec!["kubectl-tail", "pod/my-pod", "--tail", "10"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.tail, Some(10));
    }

    #[test]
    fn test_cli_parsing_verbose() {
        let args = vec!["kubectl-tail", "pod/my-pod", "-v"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(cli.verbose);
    }

    #[test]
    fn test_selector_to_labels_string() {
        let mut labels = std::collections::BTreeMap::new();
        labels.insert("app".to_string(), "nginx".to_string());
        labels.insert("version".to_string(), "v1".to_string());
        let selector = LabelSelector {
            match_labels: Some(labels),
            match_expressions: None,
        };
        let result = utils::selector_to_labels_string(&selector);
        assert_eq!(result, Some("app=nginx,version=v1".to_string()));
    }

    #[test]
    fn test_selector_to_labels_string_includes_expressions() {
        let expr = LabelSelectorRequirement {
            key: "env".to_string(),
            operator: "Exists".to_string(),
            values: None,
        };
        let selector = LabelSelector {
            match_labels: None,
            match_expressions: Some(vec![expr]),
        };

        let result = utils::selector_to_labels_string(&selector);
        assert_eq!(result, Some("env".to_string()));
    }
}

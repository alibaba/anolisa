#[cfg(test)]
mod zero_compression_tests {
    use tokenless_schema::{ResponseCompressor, SchemaCompressor};
    use serde_json::json;

    #[test]
    fn test_schema_compression_no_change_returns_original() {
        let compressor = SchemaCompressor::new();

        // A simple schema that should not be modified by compression
        let schema = json!({
            "type": "string",
            "maxLength": 100
        });

        let original_text = serde_json::to_string(&schema).unwrap();
        let result = compressor.compress(&schema);
        let result_text = serde_json::to_string(&result).unwrap();

        // Verify that when no compression occurs, the original is returned
        assert_eq!(original_text, result_text);
    }

    #[test]
    fn test_response_compression_no_change_returns_original() {
        let compressor = ResponseCompressor::new();

        // A simple response that should not be modified by compression
        let response = json!({
            "simple": "value",
            "number": 42
        });

        let original_text = serde_json::to_string(&response).unwrap();
        let result = compressor.compress(&response);
        let result_text = serde_json::to_string(&result).unwrap();

        // Verify that when no compression occurs, the original is returned
        assert_eq!(original_text, result_text);
    }

    #[test]
    fn test_schema_compression_with_change_modifies_content() {
        let compressor = SchemaCompressor::new();

        // A schema with a long description that should be compressed
        let schema = json!({
            "type": "string",
            "description": "This is a very long description that exceeds the maximum length and should be truncated to save tokens.".repeat(5)
        });

        let original_text = serde_json::to_string(&schema).unwrap();
        let result = compressor.compress(&schema);
        let result_text = serde_json::to_string(&result).unwrap();

        // The content should be different after compression
        assert_ne!(original_text, result_text);
    }

    #[test]
    fn test_response_compression_with_change_modifies_content() {
        let compressor = ResponseCompressor::new();

        // A response with a long string that should be truncated
        let response = json!({
            "long_string": "This is a very long string that exceeds the maximum length and should be truncated to save tokens. ".repeat(20)
        });

        let original_text = serde_json::to_string(&response).unwrap();
        let result = compressor.compress(&response);
        let result_text = serde_json::to_string(&result).unwrap();

        // The content should be different after compression
        assert_ne!(original_text, result_text);
    }
}
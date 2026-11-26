//! Offline specification of the existing Slack/Discord risk-alert transports.
use crate::args::ExportOpenapiArgs;
use serde_json::{json, Value};

pub fn specification() -> Value {
    let integer = json!({"type":"integer", "minimum":0, "maximum":4294967295_u64});
    let embedded_summary = json!({"type":"string", "contentMediaType":"application/json",
        "contentSchema":{"$ref":"#/components/schemas/RiskThresholdBreached"}});
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Soroban Sentinel webhook contracts",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "RiskThresholdBreached is the logical risk-alert summary carried as JSON text by the current Slack and Discord transports."
        },
        "webhooks": {
            "riskThresholdBreached": {
                "post": {
                    "summary": "Receive a storage risk alert",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "$ref": "#/components/schemas/SentinelEvent"
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Alert accepted"
                        }
                    }
                }
            }
        },
        "components": {
            "schemas": {
                "ContractId": {
                    "type": "string",
                    "pattern": "^C[A-Z2-7]{55}$",
                    "description": "Stellar contract StrKey. Consumers should additionally validate its checksum."
                },
                "LedgerSequence": integer,
                "Durability": {
                    "type": "string",
                    "enum": [
                        "instance",
                        "persistent",
                        "temporary",
                        "code"
                    ],
                    "description": "Storage lifetime class using the domain's serialized spelling."
                },
                "RiskLevel": {
                    "type": "string",
                    "oneOf": [
                        {
                            "const": "OK",
                            "description": "Remaining TTL is at least the warning threshold."
                        },
                        {
                            "const": "WARN",
                            "description": "Remaining TTL is below warning and at least critical."
                        },
                        {
                            "const": "CRITICAL",
                            "description": "Positive TTL is below the critical threshold."
                        },
                        {
                            "const": "ARCHIVED",
                            "description": "The scanner reports expired or missing TTL."
                        }
                    ]
                },
                "StorageEntry": {
                    "type": "object",
                    "required": [
                        "key_identifier",
                        "entry_type",
                        "durability",
                        "byte_size",
                        "live_until_ledger",
                        "last_modified_ledger"
                    ],
                    "properties": {
                        "key_identifier": {
                            "type": "string"
                        },
                        "entry_type": {
                            "type": "string",
                            "enum": [
                                "contract_instance",
                                "contract_code",
                                "persistent_data"
                            ]
                        },
                        "durability": {
                            "$ref": "#/components/schemas/Durability"
                        },
                        "byte_size": {
                            "type": "integer",
                            "minimum": 0
                        },
                        "live_until_ledger": {
                            "$ref": "#/components/schemas/LedgerSequence"
                        },
                        "last_modified_ledger": {
                            "$ref": "#/components/schemas/LedgerSequence"
                        }
                    }
                },
                "AlertEntry": {
                    "type": "object",
                    "required": [
                        "key_identifier",
                        "status",
                        "remaining_ledgers",
                        "remaining_days"
                    ],
                    "properties": {
                        "key_identifier": {
                            "type": "string"
                        },
                        "status": {
                            "$ref": "#/components/schemas/RiskLevel"
                        },
                        "remaining_ledgers": {
                            "type": "integer"
                        },
                        "remaining_days": {
                            "type": "number",
                            "minimum": 0
                        }
                    }
                },
                "RiskThresholdBreached": {
                    "type": "object",
                    "required": [
                        "contract_id",
                        "network",
                        "overall_status",
                        "entries",
                        "omitted_entries"
                    ],
                    "properties": {
                        "contract_id": {
                            "$ref": "#/components/schemas/ContractId"
                        },
                        "network": {
                            "type": "string"
                        },
                        "overall_status": {
                            "$ref": "#/components/schemas/RiskLevel"
                        },
                        "entries": {
                            "type": "array",
                            "maxItems": 100,
                            "items": {
                                "$ref": "#/components/schemas/AlertEntry"
                            }
                        },
                        "omitted_entries": {
                            "type": "integer",
                            "minimum": 0
                        }
                    }
                },
                "SentinelEvent": {
                    "oneOf": [
                        {
                            "$ref": "#/components/schemas/SlackRiskAlert"
                        },
                        {
                            "$ref": "#/components/schemas/DiscordRiskAlert"
                        }
                    ]
                },
                "SlackRiskAlert": {
                    "type": "object",
                    "required": [
                        "text",
                        "blocks"
                    ],
                    "properties": {
                        "text": {
                            "type": "string"
                        },
                        "blocks": {
                            "type": "array",
                            "minItems": 2,
                            "maxItems": 2,
                            "prefixItems": [
                                {
                                    "type": "object",
                                    "required": [
                                        "type",
                                        "text"
                                    ],
                                    "properties": {
                                        "type": {
                                            "const": "section"
                                        },
                                        "text": {
                                            "type": "object",
                                            "required": [
                                                "type",
                                                "text"
                                            ],
                                            "properties": {
                                                "type": {
                                                    "const": "mrkdwn"
                                                },
                                                "text": {
                                                    "type": "string"
                                                }
                                            }
                                        }
                                    }
                                },
                                {
                                    "type": "object",
                                    "required": [
                                        "type",
                                        "text"
                                    ],
                                    "properties": {
                                        "type": {
                                            "const": "section"
                                        },
                                        "text": {
                                            "type": "object",
                                            "required": [
                                                "type",
                                                "text"
                                            ],
                                            "properties": {
                                                "type": {
                                                    "const": "mrkdwn"
                                                },
                                                "text": embedded_summary.clone()
                                            }
                                        }
                                    }
                                }
                            ]
                        }
                    }
                },
                "DiscordRiskAlert": {
                    "type": "object",
                    "required": [
                        "embeds"
                    ],
                    "properties": {
                        "embeds": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": 1,
                            "items": {
                                "type": "object",
                                "required": [
                                    "title",
                                    "description",
                                    "color"
                                ],
                                "properties": {
                                    "title": {
                                        "type": "string"
                                    },
                                    "description": embedded_summary,
                                    "color": {
                                        "type": "integer",
                                        "enum": [
                                            15158332,
                                            16776960
                                        ]
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

pub fn run_export_openapi(args: ExportOpenapiArgs) -> i32 {
    let document = specification();
    let output = if args.format == "yaml" {
        serde_yaml::to_string(&document).map_err(|error| error.to_string())
    } else {
        serde_json::to_string_pretty(&document).map_err(|error| error.to_string())
    };
    match output {
        Ok(text) => {
            println!("{}", text.trim_end());
            0
        }
        Err(_) => {
            eprintln!("Error: OpenAPI serialization failed");
            10
        }
    }
}

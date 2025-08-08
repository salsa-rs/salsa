#![cfg(all(feature = "persistence", feature = "inventory"))]

mod common;
use common::LogDatabase;

use expect_test::expect;

#[salsa::input(persist)]
struct MyInput {
    field: usize,
}

#[salsa::interned(persist)]
struct MyInterned<'db> {
    field: String,
}

#[salsa::tracked(persist)]
struct MyTracked<'db> {
    field: String,
}

#[salsa::tracked(persist)]
fn unit_to_interned(db: &dyn salsa::Database) -> MyInterned<'_> {
    MyInterned::new(db, "a".repeat(50))
}

#[salsa::tracked(persist)]
fn input_to_tracked(db: &dyn salsa::Database, input: MyInput) -> MyTracked<'_> {
    MyTracked::new(db, "a".repeat(input.field(db)))
}

#[salsa::tracked(persist)]
fn input_pair_to_string(db: &dyn salsa::Database, input1: MyInput, input2: MyInput) -> String {
    "a".repeat(input1.field(db) + input2.field(db))
}

#[test]
fn everything() {
    let mut db = common::LoggerDatabase::default();

    let _input1 = MyInput::new(&db, 1);
    let _input2 = MyInput::new(&db, 2);

    let serialized =
        serde_json::to_string_pretty(&<dyn salsa::Database>::as_serialize(&mut db)).unwrap();

    let expected = expect![[r#"
        {
          "runtime": {
            "revisions": [
              1,
              1,
              1
            ]
          },
          "ingredients": {
            "MyInput": {
              "1": {
                "durabilities": [
                  "Low"
                ],
                "revisions": [
                  1
                ],
                "fields": [
                  1
                ]
              },
              "2": {
                "durabilities": [
                  "Low"
                ],
                "revisions": [
                  1
                ],
                "fields": [
                  2
                ]
              }
            }
          }
        }"#]];

    expected.assert_eq(&serialized);

    let input1 = MyInput::new(&db, 1);
    let input2 = MyInput::new(&db, 2);

    let _out = unit_to_interned(&db);
    let _out = input_to_tracked(&db, input1);
    let _out = input_pair_to_string(&db, input1, input2);

    let serialized =
        serde_json::to_string_pretty(&<dyn salsa::Database>::as_serialize(&mut db)).unwrap();

    let expected = expect![[r#"
        {
          "runtime": {
            "revisions": [
              1,
              1,
              1
            ]
          },
          "ingredients": {
            "MyInput": {
              "1": {
                "durabilities": [
                  "Low"
                ],
                "revisions": [
                  1
                ],
                "fields": [
                  1
                ]
              },
              "2": {
                "durabilities": [
                  "Low"
                ],
                "revisions": [
                  1
                ],
                "fields": [
                  2
                ]
              },
              "3": {
                "durabilities": [
                  "Low"
                ],
                "revisions": [
                  1
                ],
                "fields": [
                  1
                ]
              },
              "4": {
                "durabilities": [
                  "Low"
                ],
                "revisions": [
                  1
                ],
                "fields": [
                  2
                ]
              }
            },
            "MyInterned": {
              "2049": {
                "durability": "High",
                "last_interned_at": 1,
                "fields": [
                  "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                ]
              }
            },
            "MyTracked": {
              "3073": {
                "durability": "Low",
                "updated_at": 1,
                "revisions": [],
                "fields": [
                  "a"
                ]
              }
            },
            "input_pair_to_string::interned_arguments": {
              "4097": {
                "durability": "High",
                "last_interned_at": 18446744073709551615,
                "fields": [
                  {
                    "index": 3,
                    "generation": 0
                  },
                  {
                    "index": 4,
                    "generation": 0
                  }
                ]
              }
            },
            "unit_to_interned::interned_arguments": {
              "1025": {
                "durability": "High",
                "last_interned_at": 18446744073709551615,
                "fields": null
              }
            },
            "input_pair_to_string": {
              "9:4097": {
                "value": "aaa",
                "verified_at": 1,
                "revisions": {
                  "changed_at": 1,
                  "durability": "Low",
                  "origin": {
                    "Derived": [
                      {
                        "key": {
                          "key_index": {
                            "index": 3,
                            "generation": 0
                          },
                          "ingredient_index": 1
                        }
                      },
                      {
                        "key": {
                          "key_index": {
                            "index": 4,
                            "generation": 0
                          },
                          "ingredient_index": 1
                        }
                      }
                    ]
                  },
                  "verified_final": true,
                  "extra": null
                }
              }
            },
            "input_to_tracked": {
              "0:3": {
                "value": {
                  "index": 3073,
                  "generation": 0
                },
                "verified_at": 1,
                "revisions": {
                  "changed_at": 1,
                  "durability": "Low",
                  "origin": {
                    "Derived": [
                      {
                        "key": {
                          "key_index": {
                            "index": 3,
                            "generation": 0
                          },
                          "ingredient_index": 1
                        }
                      },
                      {
                        "key": {
                          "key_index": {
                            "index": 3073,
                            "generation": 0
                          },
                          "ingredient_index": 2147483651
                        }
                      }
                    ]
                  },
                  "verified_final": true,
                  "extra": {
                    "tracked_struct_ids": [
                      [
                        {
                          "ingredient_index": 3,
                          "hash": 6073466998405137972,
                          "disambiguator": 0
                        },
                        {
                          "index": 3073,
                          "generation": 0
                        }
                      ]
                    ],
                    "cycle_heads": [],
                    "iteration": 0
                  }
                }
              }
            },
            "unit_to_interned": {
              "6:1025": {
                "value": {
                  "index": 2049,
                  "generation": 0
                },
                "verified_at": 1,
                "revisions": {
                  "changed_at": 1,
                  "durability": "High",
                  "origin": {
                    "Derived": [
                      {
                        "key": {
                          "key_index": {
                            "index": 2049,
                            "generation": 0
                          },
                          "ingredient_index": 2
                        }
                      }
                    ]
                  },
                  "verified_final": true,
                  "extra": null
                }
              }
            }
          }
        }"#]];

    expected.assert_eq(&serialized);

    let mut db = common::EventLoggerDatabase::default();
    <dyn salsa::Database>::deserialize(
        &mut db,
        &mut serde_json::Deserializer::from_str(&serialized),
    )
    .unwrap();

    let _out = unit_to_interned(&db);
    let _out = input_to_tracked(&db, input1);
    let _out = input_pair_to_string(&db, input1, input2);

    // The structs are not recreated, and the queries are not reexecuted.
    db.assert_logs(expect![[r#"
        [
            "DidSetCancellationFlag",
            "WillCheckCancellation",
            "WillCheckCancellation",
            "WillCheckCancellation",
        ]"#]]);
}

#[test]
#[should_panic(expected = "is not persistable")]
fn invalid_dependency() {
    #[salsa::interned]
    struct MyInterned<'db> {
        field: usize,
    }

    #[salsa::tracked(persist)]
    fn new_interned(db: &dyn salsa::Database) {
        let _interned = MyInterned::new(db, 0);
    }

    let mut db = common::LoggerDatabase::default();

    new_interned(&db);

    let _serialized =
        serde_json::to_string_pretty(&<dyn salsa::Database>::as_serialize(&mut db)).unwrap();
}

#[test]
fn serialize_nothing() {
    let mut db = common::LoggerDatabase::default();

    let serialized =
        serde_json::to_string_pretty(&<dyn salsa::Database>::as_serialize(&mut db)).unwrap();

    // Empty ingredients should not be serialized.
    let expected = expect![[r#"
        {
          "runtime": {
            "revisions": [
              1,
              1,
              1
            ]
          },
          "ingredients": {}
        }"#]];

    expected.assert_eq(&serialized);
}

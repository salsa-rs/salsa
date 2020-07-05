# Diagram

Based on the hello world example:

```rust,ignore
{{#include ../../../examples/hello_world/main.rs:trait}}
```

```rust,ignore
{{#include ../../../examples/hello_world/main.rs:database}}
```

```mermaid
graph LR
    classDef diagramNode text-align:left;
    subgraph query group
        HelloWorldTrait["trait HelloWorld: Database + HasQueryGroup(HelloWorldStroage)"]
        HelloWorldImpl["impl(DB) HelloWorld for DB<br>where DB: HasQueryGroup(HelloWorldStorage)"]
        click HelloWorldImpl "http:query_groups.html#impl-of-the-hello-world-trait" "more info"
        HelloWorldStorage["struct HelloWorldStorage"]
        click HelloWorldStorage "http:query_groups.html#the-group-struct-and-querygroup-trait" "more info"
        QueryGroupImpl["impl QueryGroup for HelloWorldStorage<br>&nbsp;&nbsp;type DynDb = dyn HelloWorld<br>&nbsp;&nbsp;type Storage = HelloWorldGroupStorage__;"]
        click QueryGroupImpl "http:query_groups.html#the-group-struct-and-querygroup-trait" "more info"
        HelloWorldGroupStorage["struct HelloWorldGroupStorage__"]
        click HelloWorldGroupStorage "http:query_groups.html#group-storage" "more info"
        subgraph for each query...
            LengthQuery[struct LengthQuery]
            LengthQueryImpl["impl Query for LengthQuery<br>&nbsp;&nbsp;type Key = ()<br>&nbsp;&nbsp;type Value = usize<br>&nbsp;&nbsp;type Storage = salsa::DerivedStorage(Self)<br>&nbsp;&nbsp;type QueryGroup = HelloWorldStorage"]
            LengthQueryFunctionImpl["impl QueryFunction for LengthQuery<br>&nbsp;&nbsp;fn execute(db: &dyn HelloWorld, key: ()) -> usize"]
            click LengthQuery "http:query_groups.html#for-each-query-a-query-struct" "more info"
            click LengthQueryImpl "http:query_groups.html#for-each-query-a-query-struct" "more info"
            click LengthQueryFunctionImpl "http:query_groups.html#for-each-query-a-query-struct" "more info"
        end
        class HelloWorldTrait,HelloWorldImpl,HelloWorldStorage,QueryGroupImpl,HelloWorldGroupStorage diagramNode;
        class LengthQuery,LengthQueryImpl,LengthQueryFunctionImpl diagramNode;
    end
    subgraph database
        DatabaseStruct["struct Database { .. storage: Storage(Self) .. }"]
        subgraph for each group...
            HasQueryGroup["impl plumbing::HasQueryGroup(HelloWorldStorage) for DatabaseStruct"]
            click HasQueryGroup "http:database.html#the-hasquerygroup-impl" "more info"
        end
        DatabaseStorageTypes["impl plumbing::DatabaseStorageTypes for DatabaseStruct<br>&nbsp;&nbsp;type DatabaseStorage = __SalsaDatabaseStorage"]
        click DatabaseStorageTypes "http:database.html#the-databasestoragetypes-impl" "more info"
        DatabaseStorage["struct __SalsaDatabaseStorage"]
        click DatabaseStorage "http:database.html#the-database-storage-struct" "more info"
        DatabaseOps["impl plumbing::DatabaseOps for DatabaseStruct"]
        click DatabaseOps "http:database.html#the-databaseops-impl" "more info"
        class DatabaseStruct,DatabaseStorage,DatabaseStorageTypes,DatabaseOps,HasQueryGroup diagramNode;
    end
    subgraph salsa crate
        DerivedStorage["DerivedStorage"]
        class DerivedStorage diagramNode;
    end
    LengthQueryImpl --> DerivedStorage;
    DatabaseStruct --> HelloWorldImpl
    HasQueryGroup --> HelloWorldImpl
```
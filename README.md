
# mysql sql parser for rust
More sqlparse-rs to extend mysql partial syntax.   
the extended syntax is as follows

 1. variable name starting with @ sign. for example, select @@version
 2. mysql limit syntax, such as select * from t1 limit 1,2
 3. support mysql call syntax
 4. lock tables and unlock tables
 5. support setting variables separated by spaces, such as set names utf8
 6. support setting temporary variables, such as set @a=1
 7. use database
 8. support keyword backticks

 

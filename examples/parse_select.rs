// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![warn(clippy::all)]

use sqlparser::dialect::MySqlDialect;
use sqlparser::parser::*;

fn main() {
    // let sql = "insert into tbl_activeuser_trace(minute, count) value(?, ?)";
    // let sql = "insert into tbl_activeuser_trace(id, id1) values(?,?),(?,?)";
    // let sql = "update t1 set a = ? where b = ?";
    let sql = "insert a(id) value(1)";
    //let sql = "";
    let dialect = MySqlDialect {};

    let ast = Parser::parse_sql(&dialect, sql).unwrap();
    println!("AST: {:?}", ast);
    for i in ast{
        println!("{:?}", i.to_string());
    }
}

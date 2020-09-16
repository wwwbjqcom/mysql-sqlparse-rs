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
    let sql = "CREATE TABLE user_proxy_code (
  id int NOT NULL AUTO_INCREMENT,
  user_id int NOT NULL COMMENT '用户的ID',
  proxy_code int DEFAULT NULL COMMENT '邀请人ID',
  date int DEFAULT NULL COMMENT '时间',
  PRIMARY KEY (id) USING BTREE,
  KEY index_user_id (user_id) comment 'abc'
) ENGINE=InnoDB AUTO_INCREMENT=239 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;";
    //let sql = "";
    let dialect = MySqlDialect {};

    let ast = Parser::parse_sql(&dialect, sql).unwrap();

    println!("AST: {:?}", ast[0].to_string());

}

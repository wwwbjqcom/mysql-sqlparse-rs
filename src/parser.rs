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

//! SQL Parser

use log::debug;

use super::ast::*;
use super::dialect::keywords;
use super::dialect::keywords::Keyword;
use super::dialect::Dialect;
use super::tokenizer::*;
use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum ParserError {
    TokenizerError(String),
    ParserError(String),
}

// Use `Parser::expected` instead, if possible
macro_rules! parser_err {
    ($MSG:expr) => {
        Err(ParserError::ParserError($MSG.to_string()))
    };
}

// Returns a successful result if the optional expression is some
macro_rules! return_ok_if_some {
    ($e:expr) => {{
        if let Some(v) = $e {
            return Ok(v);
        }
    }};
}

#[derive(PartialEq)]
pub enum IsOptional {
    Optional,
    Mandatory,
}
use IsOptional::*;

pub enum IsLateral {
    Lateral,
    NotLateral,
}
use crate::ast::Statement::CreateVirtualTable;
use IsLateral::*;
use crate::dialect::DBType;
use crate::ast::Expr::Exists;


impl From<TokenizerError> for ParserError {
    fn from(e: TokenizerError) -> Self {
        ParserError::TokenizerError(format!(
            "{} at Line: {}, Column {}",
            e.message, e.line, e.col
        ))
    }
}

impl fmt::Display for ParserError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "sql parser error: {}",
            match self {
                ParserError::TokenizerError(s) => s,
                ParserError::ParserError(s) => s,
            }
        )
    }
}

impl Error for ParserError {}



/// SQL Parser
pub struct Parser {
    tokens: Vec<Token>,
    /// The index of the first unprocessed token in `self.tokens`
    index: usize,

    dialect_type: DBType
}

impl Parser {
    /// Parse the specified tokens
    pub fn new(tokens: Vec<Token>, db_type : DBType) -> Self {
        Parser { tokens, index: 0 , dialect_type: db_type}
    }

    /// Parse a SQL statement and produce an Abstract Syntax Tree (AST)
    pub fn parse_sql(dialect: &dyn Dialect, sql: &str) -> Result<Vec<Statement>, ParserError> {
        let mut tokenizer = Tokenizer::new(dialect, &sql);
        let tokens = tokenizer.tokenize()?;
        // println!("Parsing sql tokens '{:?}'...", &tokens);
        let mut parser = Parser::new(tokens, dialect.check_db_type());
        let mut stmts = Vec::new();
        let mut expecting_statement_delimiter = false;
        debug!("Parsing sql '{}'...", sql);
        loop {
            // ignore empty statements (between successive statement delimiters)
            while parser.consume_token(&Token::SemiColon) {
                expecting_statement_delimiter = false;
            }

            if parser.peek_token() == Token::EOF {
                break;
            }
            if expecting_statement_delimiter {
                return parser.expected("end of statement", parser.peek_token());
            }

            let statement = parser.parse_statement()?;
            stmts.push(statement);
            expecting_statement_delimiter = true;
        }
        Ok(stmts)
    }

    /// Parse a single top-level statement (such as SELECT, INSERT, CREATE, etc.),
    /// stopping before the statement separator, if any.
    pub fn parse_statement(&mut self) -> Result<Statement, ParserError> {
        //println!("{:?}", self.peek_token());
        match self.next_token() {
            Token::Word(w) => match w.keyword {
                Keyword::SELECT | Keyword::WITH | Keyword::VALUES => {
                    self.prev_token();
                    Ok(Statement::Query(Box::new(self.parse_query()?)))
                }
                Keyword::EXPLAIN => Ok(self.parse_explain()?),
                Keyword::CALL => Ok(self.parse_call()?),
                Keyword::CREATE => Ok(self.parse_create()?),
                Keyword::DROP => Ok(self.parse_drop()?),
                Keyword::DELETE => Ok(self.parse_delete()?),
                Keyword::INSERT => Ok(self.parse_insert()?),
                Keyword::REPLACE => Ok(self.parse_insert()?),
                Keyword::RELOAD => Ok(self.parse_reload()?),
                Keyword::UPDATE => Ok(self.parse_update()?),
                Keyword::ALTER => Ok(self.parse_alter()?),
                Keyword::COPY => Ok(self.parse_copy()?),
                Keyword::SET => Ok(self.parse_set()?),
                Keyword::SHOW => Ok(self.parse_show()?),
                Keyword::START => Ok(self.parse_start_transaction()?),
                // `BEGIN` is a nonstandard but common alias for the
                // standard `START TRANSACTION` statement. It is supported
                // by at least PostgreSQL and MySQL.
                Keyword::BEGIN => Ok(self.parse_begin()?),
                Keyword::COMMIT => Ok(self.parse_commit()?),
                Keyword::ROLLBACK => Ok(self.parse_rollback()?),
                Keyword::ASSERT => Ok(self.parse_assert()?),
                Keyword::LOCK => Ok(self.parse_lock()?),
                Keyword::UNLOCK => Ok(self.parse_unlock()?),
                Keyword::USE => Ok(self.parse_use()?),
                Keyword::DESC => Ok(self.parse_desc()?),
                _ => self.expected("an SQL statement", Token::Word(w)),
            },
            Token::LParen => {
                self.prev_token();
                Ok(Statement::Query(Box::new(self.parse_query()?)))
            }
            unexpected => self.expected("an SQL statement", unexpected),
        }
    }


    pub fn parse_explain(&mut self) -> Result<Statement, ParserError>{
        let analyze = self.parse_explain_analyze()?;
        let format_type = self.parse_explain_format()?;
        let body = match self.next_token(){
            Token::Word(w) => match w.keyword {
                Keyword::SELECT | Keyword::WITH | Keyword::VALUE => {
                    self.prev_token();
                    Ok(ExplainStmt::Stmt(Box::new(Statement::Query(Box::new(self.parse_query()?)))))
                }
                Keyword::UPDATE => Ok(ExplainStmt::Stmt(Box::new(self.parse_update()?))),
                Keyword::DELETE => Ok(ExplainStmt::Stmt(Box::new(self.parse_delete()?))),
                Keyword::FOR => Ok(self.parse_explain_for_connection()?),
                _ => self.expected("Explain explainable_stmt ", Token::Word(w))
            }
            unexpected => self.expected("Explain explainable_stmt ", unexpected),
        }?;
        Ok(Statement::Explain { analyze, format_type, body })
    }

    pub fn parse_explain_for_connection(&mut self) -> Result<ExplainStmt, ParserError>{
        if self.parse_keyword(Keyword::CONNECTION){
            let token = self.peek_token();
            let value = match (self.parse_value(), token) {
                (Ok(value), _) => ExplainStmt::Connection(value),
                (Err(_), unexpected) => self.expected("connection value", unexpected)?,
            };
            Ok(value)
        }else {
            self.expected("EXPLAIN FOR  ", self.peek_token())
        }
    }

    pub fn parse_explain_analyze(&mut self) -> Result<Option<bool>, ParserError>{
        if self.parse_keyword(Keyword::ANALYZE){
            Ok(Some(true))
        }else {
            Ok(None)
        }

    }

    pub fn parse_explain_format(&mut self) -> Result<Option<ExplainFormat>, ParserError>{
        if self.parse_keyword(Keyword::FORMAT){
            if self.consume_token(&Token::Eq){
                if self.parse_keyword(Keyword::JSON) {
                    Ok(Some(ExplainFormat::JSON))
                }else if self.parse_keyword(Keyword::TRADITIONAL) {
                    Ok(Some(ExplainFormat::TRADITIONAL))
                }else if self.parse_keyword(Keyword::TREE) {
                    Ok(Some(ExplainFormat::TREE))
                }
                else {
                    self.expected("EXPLAIN FORMAT =", self.peek_token())
                }
            }else {
                self.expected("EXPLAIN FORMAT =", self.peek_token())
            }
        }else {
            Ok(None)
        }


    }

    pub fn parse_desc(&mut self) -> Result<Statement, ParserError>{
        let table_name = self.parse_object_name()?;
        Ok(Statement::Desc { table_name })
    }

    pub fn parse_use(&mut self) -> Result<Statement, ParserError> {
        let database_name = self.parse_identifier()?;
        if self.consume_token(&Token::EOF){
            return Ok(Statement::ChangeDatabase {database: database_name.to_string()});
        }
        return self.expected(
            "Use Wrong syntax",
            self.peek_token(),
        );
    }

    pub fn parse_call(&mut self) -> Result<Statement, ParserError>{
        let fun_name = self.parse_identifier()?;
        return if self.consume_token(&Token::EOF) {
            Ok(Statement::Call { name: fun_name, parameter: None })
        } else {
            Ok(Statement::Call { name: fun_name, parameter: Some(self.parse_call_parameter()?) })
        }

    }

    pub fn parse_call_parameter(&mut self) -> Result<Vec<Expr>, ParserError> {
        let mut ident_list = vec![];
        let mut r_paren =  false;
        if self.consume_token(&Token::LParen){
            loop {
                if self.consume_token(&Token::RParen){
                    r_paren = true;
                    continue;
                }

                if r_paren && self.consume_token(&Token::EOF){
                    break;
                }else if r_paren {
                    return self.expected(
                        "Call Wrong syntax",
                        self.peek_token(),
                    );
                }

                ident_list = self.parse_comma_separated(Parser::parse_expr)?;

            }
        }else {
            return self.expected(
                "Call Wrong syntax",
                self.peek_token(),
            );
        }
        return Ok(ident_list)
    }

    pub fn parse_unlock(&mut self) -> Result<Statement, ParserError>{
        if self.parse_keyword(Keyword::TABLES) && self.consume_token(&Token::EOF){
            return Ok(Statement::UNLock { chain: true })
        }
        return self.expected(
            "UNLOCK Wrong syntax",
            self.peek_token(),
        );

    }

    pub fn parse_lock(&mut self) -> Result<Statement, ParserError>{
        return Ok(Statement::Lock { lock_tables: self.parse_lock_tables_info()?})
    }

    pub fn parse_lock_tables_info(&mut self) -> Result<Vec<LockInfo>, ParserError>{
        if self.parse_keyword(Keyword::TABLES) {
            let mut lock_list = vec![];
            loop{
                lock_list.push(self.parse_lock_tables_relation()?);
                if !self.consume_token(&Token::Comma){
                    break
                }
            }
            Ok(lock_list)
        }else {
            return self.expected(
                "LOCK Wrong syntax",
                self.peek_token(),
            );
        }
    }

    pub fn parse_lock_tables_relation(&mut self) -> Result<LockInfo, ParserError> {
        let name = self.parse_object_name()?;
        let lock_type = self.parse_lock_type()?;
        return Ok(LockInfo{ table_name: name, lock: lock_type });
    }

    /// 解析获取lock类型
    pub fn parse_lock_type(&mut self) -> Result<LOCKType, ParserError> {
        match self.peek_token(){
            Token::Word(_f) => {
                if self.parse_keyword(Keyword::READ){
                    return Ok(LOCKType::Read);
                }else if self.parse_keyword(Keyword::WRITE){
                    return Ok(LOCKType::Write);
                }
            }
            _ => {}
        }
        return self.expected(
            "LOCK tables type only support read and write",
            self.peek_token(),
        );
    }

    /// Parse a new expression
    pub fn parse_expr(&mut self) -> Result<Expr, ParserError> {
        self.parse_subexpr(0)
    }

    /// Parse tokens until the precedence changes
    pub fn parse_subexpr(&mut self, precedence: u8) -> Result<Expr, ParserError> {
        debug!("parsing expr");
        let mut expr = self.parse_prefix()?;
        debug!("prefix: {:?}", expr);
        loop {
            let next_precedence = self.get_next_precedence()?;
            debug!("next precedence: {:?}", next_precedence);
            if precedence >= next_precedence {
                break;
            }

            expr = self.parse_infix(expr, next_precedence)?;
        }
        Ok(expr)
    }
    pub fn parse_assert(&mut self) -> Result<Statement, ParserError> {
        let condition = self.parse_expr()?;
        let message = if self.parse_keyword(Keyword::AS) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        Ok(Statement::Assert { condition, message })
    }

    /// Parse an expression prefix
    pub fn parse_prefix(&mut self) -> Result<Expr, ParserError> {
        // PostgreSQL allows any string literal to be preceded by a type name, indicating that the
        // string literal represents a literal of that type. Some examples:
        //
        //      DATE '2020-05-20'
        //      TIMESTAMP WITH TIME ZONE '2020-05-20 7:43:54'
        //      BOOL 'true'
        //
        // The first two are standard SQL, while the latter is a PostgreSQL extension. Complicating
        // matters is the fact that INTERVAL string literals may optionally be followed by special
        // keywords, e.g.:
        //
        //      INTERVAL '7' DAY
        //
        // Note also that naively `SELECT date` looks like a syntax error because the `date` type
        // name is not followed by a string literal, but in fact in PostgreSQL it is a valid
        // expression that should parse as the column name "date".
        return_ok_if_some!(self.maybe_parse(|parser| {
            match parser.parse_data_type()? {
                DataType::Interval => parser.parse_literal_interval(),
                // PosgreSQL allows almost any identifier to be used as custom data type name,
                // and we support that in `parse_data_type()`. But unlike Postgres we don't
                // have a list of globally reserved keywords (since they vary across dialects),
                // so given `NOT 'a' LIKE 'b'`, we'd accept `NOT` as a possible custom data type
                // name, resulting in `NOT 'a'` being recognized as a `TypedString` instead of
                // an unary negation `NOT ('a' LIKE 'b')`. To solve this, we don't accept the
                // `type 'string'` syntax for the custom data types at all.
                DataType::Custom(..) => parser_err!("dummy"),
                data_type => Ok(Expr::TypedString {
                    data_type,
                    value: parser.parse_literal_string()?,
                }),
            }
        }));

        let expr = match self.next_token() {
            Token::Word(w) => match w.keyword {
                Keyword::TRUE | Keyword::FALSE | Keyword::NULL => {
                    self.prev_token();
                    Ok(Expr::Value(self.parse_value()?))
                }
                Keyword::CASE => self.parse_case_expr(),
                Keyword::CAST => self.parse_cast_expr(),
                Keyword::EXISTS => self.parse_exists_expr(),
                Keyword::EXTRACT => self.parse_extract_expr(),
                Keyword::INTERVAL => self.parse_literal_interval(),
                Keyword::LISTAGG => self.parse_listagg_expr(),
                Keyword::NOT => Ok(Expr::UnaryOp {
                    op: UnaryOperator::Not,
                    expr: Box::new(self.parse_subexpr(Self::UNARY_NOT_PREC)?),
                }),
                // Here `w` is a word, check if it's a part of a multi-part
                // identifier, a function call, or a simple identifier:
                _ => match self.peek_token() {
                    Token::LParen | Token::Period => {
                        let mut id_parts: Vec<Ident> = vec![w.to_ident()];
                        let mut ends_with_wildcard = false;
                        while self.consume_token(&Token::Period) {
                            match self.next_token() {
                                Token::Word(w) => id_parts.push(w.to_ident()),
                                Token::Mult => {
                                    ends_with_wildcard = true;
                                    break;
                                }
                                unexpected => {
                                    return self
                                        .expected("an identifier or a '*' after '.'", unexpected);
                                }
                            }
                        }
                        if ends_with_wildcard {
                            Ok(Expr::QualifiedWildcard(id_parts))
                        } else if self.consume_token(&Token::LParen) {
                            self.prev_token();
                            self.parse_function(ObjectName(id_parts))
                        } else {
                            Ok(Expr::CompoundIdentifier(id_parts))
                        }
                    }
                    _ => Ok(Expr::Identifier(w.to_ident())),
                },
            }, // End of Token::Word
            Token::Mult => Ok(Expr::Wildcard),
            tok @ Token::Minus | tok @ Token::Plus => {
                let op = if tok == Token::Plus {
                    UnaryOperator::Plus
                } else {
                    UnaryOperator::Minus
                };
                Ok(Expr::UnaryOp {
                    op,
                    expr: Box::new(self.parse_subexpr(Self::PLUS_MINUS_PREC)?),
                })
            }
            Token::Number(_)
            | Token::SingleQuotedString(_)
            | Token::NationalStringLiteral(_)
            | Token::HexStringLiteral(_)
            | Token::VariableString(_)
            | Token::Char(_) => {
                self.prev_token();
                Ok(Expr::Value(self.parse_value()?))
            }
            Token::LParen => {
                let expr =
                    if self.parse_keyword(Keyword::SELECT) || self.parse_keyword(Keyword::WITH) {
                        self.prev_token();
                        Expr::Subquery(Box::new(self.parse_query()?))
                    } else {
                        Expr::Nested(Box::new(self.parse_expr()?))
                    };
                self.expect_token(&Token::RParen)?;
                Ok(expr)
            }
            Token::Negate => {
                Ok(Expr::BitwiseNested(Box::new(self.parse_expr()?)))
            }
            unexpected => self.expected("an expression", unexpected),
        }?;

        match &self.dialect_type{
            DBType::MySql => Ok(expr),
            _ => {
                return if self.parse_keyword(Keyword::COLLATE) {
                    Ok(Expr::Collate {
                        expr: Box::new(expr),
                        collation: self.parse_object_name()?,
                    })
                } else {
                    Ok(expr)
                }
            }
        }

    }

    pub fn parse_function(&mut self, name: ObjectName) -> Result<Expr, ParserError> {
        self.expect_token(&Token::LParen)?;
        let distinct = self.parse_all_or_distinct()?;
        let args = self.parse_optional_args()?;
        let over = if self.parse_keyword(Keyword::OVER) {
            // TBD: support window names (`OVER mywin`) in place of inline specification
            self.expect_token(&Token::LParen)?;
            let partition_by = if self.parse_keywords(&[Keyword::PARTITION, Keyword::BY]) {
                // a list of possibly-qualified column names
                self.parse_comma_separated(Parser::parse_expr)?
            } else {
                vec![]
            };
            let order_by = if self.parse_keywords(&[Keyword::ORDER, Keyword::BY]) {
                self.parse_comma_separated(Parser::parse_order_by_expr)?
            } else {
                vec![]
            };
            let window_frame = if !self.consume_token(&Token::RParen) {
                let window_frame = self.parse_window_frame()?;
                self.expect_token(&Token::RParen)?;
                Some(window_frame)
            } else {
                None
            };

            Some(WindowSpec {
                partition_by,
                order_by,
                window_frame,
            })
        } else {
            None
        };

        Ok(Expr::Function(Function {
            name,
            args,
            over,
            distinct,
        }))
    }

    pub fn parse_window_frame_units(&mut self) -> Result<WindowFrameUnits, ParserError> {
        match self.next_token() {
            Token::Word(w) => match w.keyword {
                Keyword::ROWS => Ok(WindowFrameUnits::Rows),
                Keyword::RANGE => Ok(WindowFrameUnits::Range),
                Keyword::GROUPS => Ok(WindowFrameUnits::Groups),
                _ => self.expected("ROWS, RANGE, GROUPS", Token::Word(w))?,
            },
            unexpected => self.expected("ROWS, RANGE, GROUPS", unexpected),
        }
    }

    pub fn parse_window_frame(&mut self) -> Result<WindowFrame, ParserError> {
        let units = self.parse_window_frame_units()?;
        let (start_bound, end_bound) = if self.parse_keyword(Keyword::BETWEEN) {
            let start_bound = self.parse_window_frame_bound()?;
            self.expect_keyword(Keyword::AND)?;
            let end_bound = Some(self.parse_window_frame_bound()?);
            (start_bound, end_bound)
        } else {
            (self.parse_window_frame_bound()?, None)
        };
        Ok(WindowFrame {
            units,
            start_bound,
            end_bound,
        })
    }

    /// Parse `CURRENT ROW` or `{ <positive number> | UNBOUNDED } { PRECEDING | FOLLOWING }`
    pub fn parse_window_frame_bound(&mut self) -> Result<WindowFrameBound, ParserError> {
        if self.parse_keywords(&[Keyword::CURRENT, Keyword::ROW]) {
            Ok(WindowFrameBound::CurrentRow)
        } else {
            let rows = if self.parse_keyword(Keyword::UNBOUNDED) {
                None
            } else {
                Some(self.parse_literal_uint()?)
            };
            if self.parse_keyword(Keyword::PRECEDING) {
                Ok(WindowFrameBound::Preceding(rows))
            } else if self.parse_keyword(Keyword::FOLLOWING) {
                Ok(WindowFrameBound::Following(rows))
            } else {
                self.expected("PRECEDING or FOLLOWING", self.peek_token())
            }
        }
    }

    pub fn parse_case_expr(&mut self) -> Result<Expr, ParserError> {
        let mut operand = None;
        if !self.parse_keyword(Keyword::WHEN) {
            operand = Some(Box::new(self.parse_expr()?));
            self.expect_keyword(Keyword::WHEN)?;
        }
        let mut conditions = vec![];
        let mut results = vec![];
        loop {
            conditions.push(self.parse_expr()?);
            self.expect_keyword(Keyword::THEN)?;
            results.push(self.parse_expr()?);
            if !self.parse_keyword(Keyword::WHEN) {
                break;
            }
        }
        let else_result = if self.parse_keyword(Keyword::ELSE) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect_keyword(Keyword::END)?;
        Ok(Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        })
    }

    /// Parse a SQL CAST function e.g. `CAST(expr AS FLOAT)`
    pub fn parse_cast_expr(&mut self) -> Result<Expr, ParserError> {
        self.expect_token(&Token::LParen)?;
        let expr = self.parse_expr()?;
        self.expect_keyword(Keyword::AS)?;
        let data_type = self.parse_data_type()?;
        self.expect_token(&Token::RParen)?;
        Ok(Expr::Cast {
            expr: Box::new(expr),
            data_type,
        })
    }

    /// Parse a SQL EXISTS expression e.g. `WHERE EXISTS(SELECT ...)`.
    pub fn parse_exists_expr(&mut self) -> Result<Expr, ParserError> {
        self.expect_token(&Token::LParen)?;
        let exists_node = Expr::Exists(Box::new(self.parse_query()?));
        self.expect_token(&Token::RParen)?;
        Ok(exists_node)
    }

    pub fn parse_extract_expr(&mut self) -> Result<Expr, ParserError> {
        self.expect_token(&Token::LParen)?;
        let field = self.parse_date_time_field()?;
        self.expect_keyword(Keyword::FROM)?;
        let expr = self.parse_expr()?;
        self.expect_token(&Token::RParen)?;
        Ok(Expr::Extract {
            field,
            expr: Box::new(expr),
        })
    }

    /// Parse a SQL LISTAGG expression, e.g. `LISTAGG(...) WITHIN GROUP (ORDER BY ...)`.
    pub fn parse_listagg_expr(&mut self) -> Result<Expr, ParserError> {
        self.expect_token(&Token::LParen)?;
        let distinct = self.parse_all_or_distinct()?;
        let expr = Box::new(self.parse_expr()?);
        // While ANSI SQL would would require the separator, Redshift makes this optional. Here we
        // choose to make the separator optional as this provides the more general implementation.
        let separator = if self.consume_token(&Token::Comma) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        let on_overflow = if self.parse_keywords(&[Keyword::ON, Keyword::OVERFLOW]) {
            if self.parse_keyword(Keyword::ERROR) {
                Some(ListAggOnOverflow::Error)
            } else {
                self.expect_keyword(Keyword::TRUNCATE)?;
                let filler = match self.peek_token() {
                    Token::Word(w)
                        if w.keyword == Keyword::WITH || w.keyword == Keyword::WITHOUT =>
                    {
                        None
                    }
                    Token::SingleQuotedString(_)
                    | Token::NationalStringLiteral(_)
                    | Token::HexStringLiteral(_) => Some(Box::new(self.parse_expr()?)),
                    unexpected => {
                        self.expected("either filler, WITH, or WITHOUT in LISTAGG", unexpected)?
                    }
                };
                let with_count = self.parse_keyword(Keyword::WITH);
                if !with_count && !self.parse_keyword(Keyword::WITHOUT) {
                    self.expected("either WITH or WITHOUT in LISTAGG", self.peek_token())?;
                }
                self.expect_keyword(Keyword::COUNT)?;
                Some(ListAggOnOverflow::Truncate { filler, with_count })
            }
        } else {
            None
        };
        self.expect_token(&Token::RParen)?;
        // Once again ANSI SQL requires WITHIN GROUP, but Redshift does not. Again we choose the
        // more general implementation.
        let within_group = if self.parse_keywords(&[Keyword::WITHIN, Keyword::GROUP]) {
            self.expect_token(&Token::LParen)?;
            self.expect_keywords(&[Keyword::ORDER, Keyword::BY])?;
            let order_by_expr = self.parse_comma_separated(Parser::parse_order_by_expr)?;
            self.expect_token(&Token::RParen)?;
            order_by_expr
        } else {
            vec![]
        };
        Ok(Expr::ListAgg(ListAgg {
            distinct,
            expr,
            separator,
            on_overflow,
            within_group,
        }))
    }

    // This function parses date/time fields for both the EXTRACT function-like
    // operator and interval qualifiers. EXTRACT supports a wider set of
    // date/time fields than interval qualifiers, so this function may need to
    // be split in two.
    pub fn parse_date_time_field(&mut self) -> Result<DateTimeField, ParserError> {
        match self.next_token() {
            Token::Word(w) => match w.keyword {
                Keyword::YEAR => Ok(DateTimeField::Year),
                Keyword::MONTH => Ok(DateTimeField::Month),
                Keyword::DAY => Ok(DateTimeField::Day),
                Keyword::HOUR => Ok(DateTimeField::Hour),
                Keyword::MINUTE => Ok(DateTimeField::Minute),
                Keyword::SECOND => Ok(DateTimeField::Second),
                _ => self.expected("date/time field", Token::Word(w))?,
            },
            unexpected => self.expected("date/time field", unexpected),
        }
    }

    /// Parse an INTERVAL literal.
    ///
    /// Some syntactically valid intervals:
    ///
    ///   1. `INTERVAL '1' DAY`
    ///   2. `INTERVAL '1-1' YEAR TO MONTH`
    ///   3. `INTERVAL '1' SECOND`
    ///   4. `INTERVAL '1:1:1.1' HOUR (5) TO SECOND (5)`
    ///   5. `INTERVAL '1.1' SECOND (2, 2)`
    ///   6. `INTERVAL '1:1' HOUR (5) TO MINUTE (5)`
    ///
    /// Note that we do not currently attempt to parse the quoted value.
    pub fn parse_literal_interval(&mut self) -> Result<Expr, ParserError> {
        // The SQL standard allows an optional sign before the value string, but
        // it is not clear if any implementations support that syntax, so we
        // don't currently try to parse it. (The sign can instead be included
        // inside the value string.)

        // The first token in an interval is a string literal which specifies
        // the duration of the interval.
        let value = self.parse_literal_string()?;

        // Following the string literal is a qualifier which indicates the units
        // of the duration specified in the string literal.
        //
        // Note that PostgreSQL allows omitting the qualifier, so we provide
        // this more general implemenation.
        let leading_field = match self.peek_token() {
            Token::Word(kw)
                if [
                    Keyword::YEAR,
                    Keyword::MONTH,
                    Keyword::DAY,
                    Keyword::HOUR,
                    Keyword::MINUTE,
                    Keyword::SECOND,
                ]
                .iter()
                .any(|d| kw.keyword == *d) =>
            {
                Some(self.parse_date_time_field()?)
            }
            _ => None,
        };

        let (leading_precision, last_field, fsec_precision) =
            if leading_field == Some(DateTimeField::Second) {
                // SQL mandates special syntax for `SECOND TO SECOND` literals.
                // Instead of
                //     `SECOND [(<leading precision>)] TO SECOND[(<fractional seconds precision>)]`
                // one must use the special format:
                //     `SECOND [( <leading precision> [ , <fractional seconds precision>] )]`
                let last_field = None;
                let (leading_precision, fsec_precision) = self.parse_optional_precision_scale()?;
                (leading_precision, last_field, fsec_precision)
            } else {
                let leading_precision = self.parse_optional_precision()?;
                if self.parse_keyword(Keyword::TO) {
                    let last_field = Some(self.parse_date_time_field()?);
                    let fsec_precision = if last_field == Some(DateTimeField::Second) {
                        self.parse_optional_precision()?
                    } else {
                        None
                    };
                    (leading_precision, last_field, fsec_precision)
                } else {
                    (leading_precision, None, None)
                }
            };

        Ok(Expr::Value(Value::Interval {
            value,
            leading_field,
            leading_precision,
            last_field,
            fractional_seconds_precision: fsec_precision,
        }))
    }

    /// Parse an operator following an expression
    pub fn parse_infix(&mut self, expr: Expr, precedence: u8) -> Result<Expr, ParserError> {
        let tok = self.next_token();
        let regular_binary_operator = match &tok {
            Token::Eq => Some(BinaryOperator::Eq),
            Token::Neq => Some(BinaryOperator::NotEq),
            Token::Gt => Some(BinaryOperator::Gt),
            Token::GtEq => Some(BinaryOperator::GtEq),
            Token::Lt => Some(BinaryOperator::Lt),
            Token::LtEq => Some(BinaryOperator::LtEq),
            Token::Plus => Some(BinaryOperator::Plus),
            Token::Minus => Some(BinaryOperator::Minus),
            Token::Mult => Some(BinaryOperator::Multiply),
            Token::Mod => Some(BinaryOperator::Modulus),
            Token::StringConcat => Some(BinaryOperator::StringConcat),
            Token::Pipe => Some(BinaryOperator::BitwiseOr),
            Token::Caret => Some(BinaryOperator::BitwiseXor),
            Token::Ampersand => Some(BinaryOperator::BitwiseAnd),
            Token::Negate => Some(BinaryOperator::BitwiseNegate),
            Token::LDisplacement => Some(BinaryOperator::BitwiseNegateLDisplacement),
            Token::RDisplacement => Some(BinaryOperator::BitwiseNegateRDisplacement),
            Token::Div => Some(BinaryOperator::Divide),
            Token::Word(w) => match w.keyword {
                Keyword::AND => Some(BinaryOperator::And),
                Keyword::OR => Some(BinaryOperator::Or),
                Keyword::LIKE => Some(BinaryOperator::Like),
                Keyword::NOT => {
                    if self.parse_keyword(Keyword::LIKE) {
                        Some(BinaryOperator::NotLike)
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        };

        if let Some(op) = regular_binary_operator {
            Ok(Expr::BinaryOp {
                left: Box::new(expr),
                op,
                right: Box::new(self.parse_subexpr(precedence)?),
            })
        } else if let Token::Word(w) = &tok {
            match w.keyword {
                Keyword::IS => {
                    if self.parse_keyword(Keyword::NULL) {
                        Ok(Expr::IsNull(Box::new(expr)))
                    } else if self.parse_keywords(&[Keyword::NOT, Keyword::NULL]) {
                        Ok(Expr::IsNotNull(Box::new(expr)))
                    } else {
                        self.expected("NULL or NOT NULL after IS", self.peek_token())
                    }
                }
                Keyword::NOT | Keyword::IN | Keyword::BETWEEN => {
                    self.prev_token();
                    let negated = self.parse_keyword(Keyword::NOT);
                    if self.parse_keyword(Keyword::IN) {
                        self.parse_in(expr, negated)
                    } else if self.parse_keyword(Keyword::BETWEEN) {
                        self.parse_between(expr, negated)
                    } else {
                        self.expected("IN or BETWEEN after NOT", self.peek_token())
                    }
                }
                // Can only happen if `get_next_precedence` got out of sync with this function
                _ => panic!("No infix parser for token {:?}", tok),
            }
        } else if Token::DoubleColon == tok {
            self.parse_pg_cast(expr)
        } else {
            // Can only happen if `get_next_precedence` got out of sync with this function
            panic!("No infix parser for token {:?}", tok)
        }
    }

    /// Parses the parens following the `[ NOT ] IN` operator
    pub fn parse_in(&mut self, expr: Expr, negated: bool) -> Result<Expr, ParserError> {
        self.expect_token(&Token::LParen)?;
        let in_op = if self.parse_keyword(Keyword::SELECT) || self.parse_keyword(Keyword::WITH) {
            self.prev_token();
            Expr::InSubquery {
                expr: Box::new(expr),
                subquery: Box::new(self.parse_query()?),
                negated,
            }
        } else {
            Expr::InList {
                expr: Box::new(expr),
                list: self.parse_comma_separated(Parser::parse_expr)?,
                negated,
            }
        };
        self.expect_token(&Token::RParen)?;
        Ok(in_op)
    }

    /// Parses `BETWEEN <low> AND <high>`, assuming the `BETWEEN` keyword was already consumed
    pub fn parse_between(&mut self, expr: Expr, negated: bool) -> Result<Expr, ParserError> {
        // Stop parsing subexpressions for <low> and <high> on tokens with
        // precedence lower than that of `BETWEEN`, such as `AND`, `IS`, etc.
        let low = self.parse_subexpr(Self::BETWEEN_PREC)?;
        self.expect_keyword(Keyword::AND)?;
        let high = self.parse_subexpr(Self::BETWEEN_PREC)?;
        Ok(Expr::Between {
            expr: Box::new(expr),
            negated,
            low: Box::new(low),
            high: Box::new(high),
        })
    }

    /// Parse a postgresql casting style which is in the form of `expr::datatype`
    pub fn parse_pg_cast(&mut self, expr: Expr) -> Result<Expr, ParserError> {
        Ok(Expr::Cast {
            expr: Box::new(expr),
            data_type: self.parse_data_type()?,
        })
    }

    const UNARY_NOT_PREC: u8 = 15;
    const BETWEEN_PREC: u8 = 20;
    const PLUS_MINUS_PREC: u8 = 30;

    /// Get the precedence of the next token
    pub fn get_next_precedence(&self) -> Result<u8, ParserError> {
        let token = self.peek_token();
        debug!("get_next_precedence() {:?}", token);
        match token {
            Token::Word(w) if w.keyword == Keyword::OR => Ok(5),
            Token::Word(w) if w.keyword == Keyword::AND => Ok(10),
            Token::Word(w) if w.keyword == Keyword::NOT => match self.peek_nth_token(1) {
                // The precedence of NOT varies depending on keyword that
                // follows it. If it is followed by IN, BETWEEN, or LIKE,
                // it takes on the precedence of those tokens. Otherwise it
                // is not an infix operator, and therefore has zero
                // precedence.
                Token::Word(w) if w.keyword == Keyword::IN => Ok(Self::BETWEEN_PREC),
                Token::Word(w) if w.keyword == Keyword::BETWEEN => Ok(Self::BETWEEN_PREC),
                Token::Word(w) if w.keyword == Keyword::LIKE => Ok(Self::BETWEEN_PREC),
                _ => Ok(0),
            },
            Token::Word(w) if w.keyword == Keyword::IS => Ok(17),
            Token::Word(w) if w.keyword == Keyword::IN => Ok(Self::BETWEEN_PREC),
            Token::Word(w) if w.keyword == Keyword::BETWEEN => Ok(Self::BETWEEN_PREC),
            Token::Word(w) if w.keyword == Keyword::LIKE => Ok(Self::BETWEEN_PREC),
            Token::Eq | Token::Lt | Token::LtEq | Token::Neq | Token::Gt | Token::GtEq => Ok(20),
            Token::Pipe => Ok(21),
            Token::Caret => Ok(22),
            Token::Ampersand => Ok(23),
            Token::Plus | Token::Minus => Ok(Self::PLUS_MINUS_PREC),
            Token::Mult | Token::Div | Token::Mod | Token::StringConcat |
            Token::Negate | Token::LDisplacement | Token::RDisplacement => Ok(40),
            Token::DoubleColon => Ok(50),
            _ => Ok(0),
        }
    }

    /// Return the first non-whitespace token that has not yet been processed
    /// (or None if reached end-of-file)
    pub fn peek_token(&self) -> Token {
        self.peek_nth_token(0)
    }

    /// Return nth non-whitespace token that has not yet been processed
    pub fn peek_nth_token(&self, mut n: usize) -> Token {
        let mut index = self.index;
        loop {
            index += 1;
            match self.tokens.get(index - 1) {
                Some(Token::Whitespace(_)) => continue,
                non_whitespace => {
                    if n == 0 {
                        return non_whitespace.cloned().unwrap_or(Token::EOF);
                    }
                    n -= 1;
                }
            }
        }
    }

    /// Return the first non-whitespace token that has not yet been processed
    /// (or None if reached end-of-file) and mark it as processed. OK to call
    /// repeatedly after reaching EOF.
    pub fn next_token(&mut self) -> Token {
        loop {
            self.index += 1;
            match self.tokens.get(self.index - 1) {
                Some(Token::Whitespace(_)) => continue,
                token => return token.cloned().unwrap_or(Token::EOF),
            }
        }
    }

    /// Return the first non-whitespace token that has not yet been processed
    /// (or None if reached end-of-file) and mark it as processed. OK to call
    /// repeatedly after reaching EOF.
    pub fn next_token_no_ignore_comment(&mut self) -> Token {
        loop {
            self.index += 1;
            match self.tokens.get(self.index - 1) {
                Some(Token::Whitespace(Whitespace::SingleLineComment(_))) => continue,
                Some(Token::Whitespace(Whitespace::Space)) => continue,
                Some(Token::Whitespace(Whitespace::Newline)) => continue,
                Some(Token::Whitespace(Whitespace::Tab)) => continue,
                token => return token.cloned().unwrap_or(Token::EOF),
            }
        }
    }

    /// Return the first unprocessed token, possibly whitespace.
    pub fn next_token_no_skip(&mut self) -> Option<&Token> {
        self.index += 1;
        self.tokens.get(self.index - 1)
    }

    /// Push back the last one non-whitespace token. Must be called after
    /// `next_token()`, otherwise might panic. OK to call after
    /// `next_token()` indicates an EOF.
    pub fn prev_token(&mut self) {
        loop {
            assert!(self.index > 0);
            self.index -= 1;
            if let Some(Token::Whitespace(_)) = self.tokens.get(self.index) {
                continue;
            }
            return;
        }
    }

    /// Report unexpected token
    fn expected<T>(&self, expected: &str, found: Token) -> Result<T, ParserError> {
        parser_err!(format!("Expected {}, found: {}", expected, found))
    }

    /// Look for an expected keyword and consume it if it exists
    #[must_use]
    pub fn parse_keyword(&mut self, expected: Keyword) -> bool {
        match self.peek_token() {
            Token::Word(w) if expected == w.keyword => {
                self.next_token();
                true
            }
            _ => false,
        }
    }

    /// Look for an expected sequence of keywords and consume them if they exist
    #[must_use]
    pub fn parse_keywords(&mut self, keywords: &[Keyword]) -> bool {
        let index = self.index;
        for &keyword in keywords {
            if !self.parse_keyword(keyword) {
                //println!("parse_keywords aborting .. did not find {}", keyword);
                // reset index and return immediately
                self.index = index;
                return false;
            }
        }
        true
    }

    /// Look for one of the given keywords and return the one that matches.
    #[must_use]
    pub fn parse_one_of_keywords(&mut self, keywords: &[Keyword]) -> Option<Keyword> {
        match self.peek_token() {
            Token::Word(w) => {
                keywords
                    .iter()
                    .find(|keyword| **keyword == w.keyword)
                    .map(|keyword| {
                        self.next_token();
                        *keyword
                    })
            }
            _ => None,
        }
    }

    /// Bail out if the current token is not one of the expected keywords, or consume it if it is
    pub fn expect_one_of_keywords(&mut self, keywords: &[Keyword]) -> Result<Keyword, ParserError> {
        if let Some(keyword) = self.parse_one_of_keywords(keywords) {
            Ok(keyword)
        } else {
            let keywords: Vec<String> = keywords.iter().map(|x| format!("{:?}", x)).collect();
            self.expected(
                &format!("one of {}", keywords.join(" or ")),
                self.peek_token(),
            )
        }
    }

    /// Bail out if the current token is not an expected keyword, or consume it if it is
    pub fn expect_keyword(&mut self, expected: Keyword) -> Result<(), ParserError> {
        if self.parse_keyword(expected) {
            Ok(())
        } else {
            self.expected(format!("{:?}", &expected).as_str(), self.peek_token())
        }
    }

    /// Bail out if the following tokens are not the expected sequence of
    /// keywords, or consume them if they are.
    pub fn expect_keywords(&mut self, expected: &[Keyword]) -> Result<(), ParserError> {
        for &kw in expected {
            self.expect_keyword(kw)?;
        }
        Ok(())
    }

    /// Consume the next token if it matches the expected token, otherwise return false
    #[must_use]
    pub fn consume_token(&mut self, expected: &Token) -> bool {
        // println!("consume_token: {:?}, {:?}", self.peek_token(), &expected);
        if self.peek_token() == *expected {
            self.next_token();
            true
        } else {
            false
        }
    }

    /// Bail out if the current token is not an expected keyword, or consume it if it is
    pub fn expect_token(&mut self, expected: &Token) -> Result<(), ParserError> {
        if self.consume_token(expected) {
            Ok(())
        } else {
            self.expected(&expected.to_string(), self.peek_token())
        }
    }

    /// Parse a comma-separated list of 1+ items accepted by `F`
    pub fn parse_comma_separated<T, F>(&mut self, mut f: F) -> Result<Vec<T>, ParserError>
    where
        F: FnMut(&mut Parser) -> Result<T, ParserError>,
    {
        let mut values = vec![];
        loop {
            values.push(f(self)?);
            if !self.consume_token(&Token::Comma) {
                break;
            }
        }
        Ok(values)
    }

    /// Run a parser method `f`, reverting back to the current position
    /// if unsuccessful.
    #[must_use]
    fn maybe_parse<T, F>(&mut self, mut f: F) -> Option<T>
    where
        F: FnMut(&mut Parser) -> Result<T, ParserError>,
    {
        let index = self.index;
        if let Ok(t) = f(self) {
            Some(t)
        } else {
            self.index = index;
            None
        }
    }

    /// Parse either `ALL` or `DISTINCT`. Returns `true` if `DISTINCT` is parsed and results in a
    /// `ParserError` if both `ALL` and `DISTINCT` are fround.
    pub fn parse_all_or_distinct(&mut self) -> Result<bool, ParserError> {
        let all = self.parse_keyword(Keyword::ALL);
        let distinct = self.parse_keyword(Keyword::DISTINCT);
        if all && distinct {
            return parser_err!("Cannot specify both ALL and DISTINCT".to_string());
        } else {
            Ok(distinct)
        }
    }

    /// Parse a SQL CREATE statement
    pub fn parse_create(&mut self) -> Result<Statement, ParserError> {
        if self.parse_keyword(Keyword::TABLE) {
            self.parse_create_table()
        } else if self.parse_keyword(Keyword::INDEX) {
            self.parse_create_index(false)
        } else if self.parse_keywords(&[Keyword::UNIQUE, Keyword::INDEX]) {
            self.parse_create_index(true)
        } else if self.parse_keyword(Keyword::MATERIALIZED) || self.parse_keyword(Keyword::VIEW) {
            self.prev_token();
            self.parse_create_view()
        } else if self.parse_keyword(Keyword::EXTERNAL) {
            self.parse_create_external_table()
        } else if self.parse_keyword(Keyword::VIRTUAL) {
            self.parse_create_virtual_table()
        } else if self.parse_keyword(Keyword::SCHEMA) {
            self.parse_create_schema()
        } else if self.parse_keyword(Keyword::DATABASE) {
            self.parse_create_schema()
        }else {
            self.expected("an object type after CREATE", self.peek_token())
        }
    }

    /// SQLite-specific `CREATE VIRTUAL TABLE`
    pub fn parse_create_virtual_table(&mut self) -> Result<Statement, ParserError> {
        self.expect_keyword(Keyword::TABLE)?;
        let if_not_exists = self.parse_keywords(&[Keyword::IF, Keyword::NOT, Keyword::EXISTS]);
        let table_name = self.parse_object_name()?;
        self.expect_keyword(Keyword::USING)?;
        let module_name = self.parse_identifier()?;
        // SQLite docs note that module "arguments syntax is sufficiently
        // general that the arguments can be made to appear as column
        // definitions in a traditional CREATE TABLE statement", but
        // we don't implement that.
        let module_args = self.parse_parenthesized_column_list(Optional)?;
        Ok(CreateVirtualTable {
            name: table_name,
            if_not_exists,
            module_name,
            module_args,
        })
    }

    pub fn parse_create_schema(&mut self) -> Result<Statement, ParserError> {
        let schema_name = self.parse_object_name()?;
        Ok(Statement::CreateSchema { schema_name })
    }

    pub fn parse_create_external_table(&mut self) -> Result<Statement, ParserError> {
        self.expect_keyword(Keyword::TABLE)?;
        let table_name = self.parse_object_name()?;
        let (columns, index, constraints) = self.parse_columns()?;
        self.expect_keywords(&[Keyword::STORED, Keyword::AS])?;
        let file_format = self.parse_file_format()?;

        self.expect_keyword(Keyword::LOCATION)?;
        let location = self.parse_literal_string()?;

        Ok(Statement::CreateTable {
            name: table_name,
            columns,
            index,
            constraints,
            with_options: vec![],
            table_options: vec![],
            if_not_exists: false,
            external: true,
            file_format: Some(file_format),
            location: Some(location),
            query: None,
            without_rowid: false,
        })
    }

    pub fn parse_file_format(&mut self) -> Result<FileFormat, ParserError> {
        match self.next_token() {
            Token::Word(w) => match w.keyword {
                Keyword::AVRO => Ok(FileFormat::AVRO),
                Keyword::JSONFILE => Ok(FileFormat::JSONFILE),
                Keyword::ORC => Ok(FileFormat::ORC),
                Keyword::PARQUET => Ok(FileFormat::PARQUET),
                Keyword::RCFILE => Ok(FileFormat::RCFILE),
                Keyword::SEQUENCEFILE => Ok(FileFormat::SEQUENCEFILE),
                Keyword::TEXTFILE => Ok(FileFormat::TEXTFILE),
                _ => self.expected("fileformat", Token::Word(w)),
            },
            unexpected => self.expected("fileformat", unexpected),
        }
    }

    pub fn parse_create_view(&mut self) -> Result<Statement, ParserError> {
        let materialized = self.parse_keyword(Keyword::MATERIALIZED);
        self.expect_keyword(Keyword::VIEW)?;
        // Many dialects support `OR REPLACE` | `OR ALTER` right after `CREATE`, but we don't (yet).
        // ANSI SQL and Postgres support RECURSIVE here, but we don't support it either.
        let name = self.parse_object_name()?;
        let columns = self.parse_parenthesized_column_list(Optional)?;
        let with_options = self.parse_with_options()?;
        self.expect_keyword(Keyword::AS)?;
        let query = Box::new(self.parse_query()?);
        // Optional `WITH [ CASCADED | LOCAL ] CHECK OPTION` is widely supported here.
        Ok(Statement::CreateView {
            name,
            columns,
            query,
            materialized,
            with_options,
        })
    }

    pub fn parse_drop(&mut self) -> Result<Statement, ParserError> {
        let object_type = if self.parse_keyword(Keyword::TABLE) {
            ObjectType::Table
        } else if self.parse_keyword(Keyword::VIEW) {
            ObjectType::View
        } else if self.parse_keyword(Keyword::INDEX) {
            ObjectType::Index
        } else if self.parse_keyword(Keyword::SCHEMA) {
            ObjectType::Schema
        } else if self.parse_keyword(Keyword::DATABASE) {
            ObjectType::Schema
        }else {
            return self.expected("TABLE, VIEW, INDEX or SCHEMA after DROP", self.peek_token());
        };
        // Many dialects support the non standard `IF EXISTS` clause and allow
        // specifying multiple objects to delete in a single statement
        let if_exists = self.parse_keywords(&[Keyword::IF, Keyword::EXISTS]);
        let names = self.parse_comma_separated(Parser::parse_object_name)?;
        let mut on_info = ObjectName{ 0: vec![] };
        if let ObjectType::Index = object_type{
            if self.parse_keyword(Keyword::ON){
                on_info = self.parse_object_name()?;
            }
        }
        let cascade = self.parse_keyword(Keyword::CASCADE);
        let restrict = self.parse_keyword(Keyword::RESTRICT);
        if cascade && restrict {
            return parser_err!("Cannot specify both CASCADE and RESTRICT in DROP");
        }
        Ok(Statement::Drop {
            object_type,
            if_exists,
            names,
            on_info,
            cascade,
        })
    }

    pub fn parse_create_index(&mut self, unique: bool) -> Result<Statement, ParserError> {
        let if_not_exists = self.parse_keywords(&[Keyword::IF, Keyword::NOT, Keyword::EXISTS]);
        let index_name = self.parse_object_name()?;
        self.expect_keyword(Keyword::ON)?;
        let table_name = self.parse_object_name()?;
        let columns = self.parse_parenthesized_column_list(Mandatory)?;
        Ok(Statement::CreateIndex {
            name: index_name,
            table_name,
            columns,
            unique,
            if_not_exists,
        })
    }

    pub fn parse_create_table(&mut self) -> Result<Statement, ParserError> {
        let if_not_exists = self.parse_keywords(&[Keyword::IF, Keyword::NOT, Keyword::EXISTS]);
        let table_name = self.parse_object_name()?;
        // parse optional column list (schema)
        let (columns,index, constraints) = self.parse_columns()?;

        // SQLite supports `WITHOUT ROWID` at the end of `CREATE TABLE`
        let without_rowid = self.parse_keywords(&[Keyword::WITHOUT, Keyword::ROWID]);

        // PostgreSQL supports `WITH ( options )`, before `AS`
        let with_options = self.parse_with_options()?;
        let table_options = self.parse_table_options()?;
        // Parse optional `AS ( query )`
        let query = if self.parse_keyword(Keyword::AS) {
            Some(Box::new(self.parse_query()?))
        } else {
            None
        };

        Ok(Statement::CreateTable {
            name: table_name,
            columns,
            index,
            constraints,
            with_options,
            table_options,
            if_not_exists,
            external: false,
            file_format: None,
            location: None,
            query,
            without_rowid,
        })
    }

    fn parse_column_def(&mut self) -> Result<ColumnDef, ParserError> {
        let name = self.parse_identifier()?;
        let data_type = self.parse_data_type()?;

        // if self.parse_keyword(Keyword::AUTO_INCREMENT){
        //     auto_increment = true;
        // }

        let collation = if self.parse_keyword(Keyword::COLLATE) {
            Some(self.parse_object_name()?)
        }  else {
            None
        };
        let mut options = vec![];
        loop {
            match self.peek_token() {
                Token::EOF | Token::Comma | Token::RParen | Token::SemiColon => break,
                _ => options.push(self.parse_column_option_def()?),
            }
        }
        Ok(ColumnDef {
            name,
            data_type,
            collation,
            options,
        })
    }

    fn parse_columns(&mut self) -> Result<(Vec<ColumnDef>, Vec<IndexInfo>, Vec<TableConstraint>), ParserError> {
        let mut columns = vec![];
        let mut index = vec![];
        let mut constraints = vec![];
        if !self.consume_token(&Token::LParen) || self.consume_token(&Token::RParen) {
            return Ok((columns, index, constraints));
        }

        loop {
            match self.dialect_type{
                DBType::MySql => {
                    if let Token::Word(_) = self.peek_token() {
                        if let Some(index_def) = self.parse_create_table_for_index()?{
                            index.push(index_def);
                        }else {
                            let column_def = self.parse_column_def()?;
                            columns.push(column_def);
                        }
                    }
                    let comma = self.consume_token(&Token::Comma);
                    if self.consume_token(&Token::RParen) {
                        // allow a trailing comma, even though it's not in standard
                        break;
                    } else if !comma {
                        return self.expected("',' or ')' after column definition", self.peek_token());
                    }
                }
                _ => {
                    if let Some(constraint) = self.parse_optional_table_constraint()? {
                        constraints.push(constraint);
                    } else if let Token::Word(_) = self.peek_token() {
                        let column_def = self.parse_column_def()?;
                        columns.push(column_def);
                    } else {
                        return self.expected("column name or constraint definition", self.peek_token());
                    }
                    let comma = self.consume_token(&Token::Comma);
                    if self.consume_token(&Token::RParen) {
                        // allow a trailing comma, even though it's not in standard
                        break;
                    } else if !comma {
                        return self.expected("',' or ')' after column definition", self.peek_token());
                    }
                }
            }
        }

        Ok((columns, index, constraints))
    }

    fn parse_create_table_for_index(&mut self) -> Result<Option<IndexInfo>, ParserError>{
        let keyword_list = [Keyword::KEY, Keyword::INDEX, Keyword::PRIMARY,
            Keyword::UNIQUE, Keyword::FOREIGN, Keyword::FULLTEXT, Keyword::CONSTRAINT];
        return if let Some(_k) = self.parse_one_of_keywords(&keyword_list){
            self.prev_token();
            let constraint = self.parse_alter_index_constraint()?;
            let index_type = self.parse_alter_index_storge_type()?;
            let index = self.parse_alter_index_def()?;
            Ok(Some(IndexInfo{
                constraint,
                index_type,
                index
            }))
        }else {
            Ok(None)
        }
    }

    pub fn parse_table_options(&mut self) -> Result<Vec<TableOptionDef>, ParserError>{
        let mut table_options = vec![];
        loop{
            if self.consume_token(&Token::EOF) || self.consume_token(&Token::SemiColon){
                break
            }
            table_options.push(self.parse_table_option_def()?);
            
        }
        return Ok(table_options)
    }
    
    pub fn parse_table_option_def(&mut self) -> Result<TableOptionDef, ParserError>{
        let mut name = None;
        let option = if self.parse_keyword(Keyword::COMMENT){
            self.consume_table_option_token()?;
            TableOption::Comment(self.parse_expr()?)
        }else if self.parse_keyword(Keyword::COLLATE){
            self.consume_table_option_token()?;
            TableOption::Collate(self.parse_expr()?)
        }else if self.parse_keyword(Keyword::DEFAULT) {
            self.prev_token();
            name = Some(self.parse_identifier()?);
            if self.parse_keyword(Keyword::CHARSET){
                self.consume_table_option_token()?;
                TableOption::Charset(self.parse_expr()?)
            }else {
                return self.expected("talbe option for default charset", self.peek_token());
            }
        }else if self.parse_keyword(Keyword::AUTO_INCREMENT) {
            self.consume_table_option_token()?;
            match self.next_token(){
                Token::Number(a) => TableOption::Auto_Increment(a.parse().unwrap()),
                _ =>  return self.expected("table option for auto_increment", self.peek_token())
            }
        }else if self.parse_keyword(Keyword::ENGINE) {
            self.consume_table_option_token()?;
            TableOption::Engine(self.parse_expr()?)
        }
        else {
            return self.expected("table option", self.peek_token());
        };
        Ok(TableOptionDef{ name, option})

    }

    pub fn consume_table_option_token(&mut self) -> Result<(), ParserError>{
        return if self.consume_token(&Token::Eq) {
            Ok(())
        } else {
            self.expected("table option", self.peek_token())
        }
    }

    pub fn parse_column_option_def(&mut self) -> Result<ColumnOptionDef, ParserError> {
        let name = if self.parse_keyword(Keyword::CONSTRAINT) {
            Some(self.parse_identifier()?)
        } else {
            None
        };

        let option = if self.parse_keywords(&[Keyword::NOT, Keyword::NULL]) {
            ColumnOption::NotNull
        } else if self.parse_keyword(Keyword::AUTO_INCREMENT){
            ColumnOption::AutoIncrement
        } else if self.parse_keyword(Keyword::NULL) {
            ColumnOption::Null
        } else if self.parse_keyword(Keyword::UNSIGNED) {
            ColumnOption::Unsigned
        } else if self.parse_keyword(Keyword::COMMENT) {
            ColumnOption::Comment(self.parse_expr()?)
        } else if self.parse_keyword(Keyword::AFTER) {
            ColumnOption::After(self.parse_expr()?)
        }else if self.parse_keyword(Keyword::CHARACTER) {
            if self.parse_keyword(Keyword::SET){
                ColumnOption::Character(self.parse_expr()?)
            }else {
                return self.expected("column character set ", self.peek_token());
            }
        } else if self.parse_keyword(Keyword::COLLATE) {
            ColumnOption::Collate(self.parse_expr()?)
        } else if self.parse_keyword(Keyword::DEFAULT) {
            ColumnOption::Default(self.parse_expr()?)
        } else if self.parse_keywords(&[Keyword::PRIMARY, Keyword::KEY]) {
            ColumnOption::Unique { is_primary: true }
        } else if self.parse_keyword(Keyword::UNIQUE) {
            ColumnOption::Unique { is_primary: false }
        } else if self.parse_keyword(Keyword::REFERENCES) {
            let foreign_table = self.parse_object_name()?;
            // PostgreSQL allows omitting the column list and
            // uses the primary key column of the foreign table by default
            let referred_columns = self.parse_parenthesized_column_list(Optional)?;
            let mut on_delete = None;
            let mut on_update = None;
            loop {
                if on_delete.is_none() && self.parse_keywords(&[Keyword::ON, Keyword::DELETE]) {
                    on_delete = Some(self.parse_referential_action()?);
                } else if on_update.is_none()
                    && self.parse_keywords(&[Keyword::ON, Keyword::UPDATE])
                {
                    on_update = Some(self.parse_referential_action()?);
                } else {
                    break;
                }
            }
            ColumnOption::ForeignKey {
                foreign_table,
                referred_columns,
                on_delete,
                on_update,
            }
        } else if self.parse_keyword(Keyword::CHECK) {
            self.expect_token(&Token::LParen)?;
            let expr = self.parse_expr()?;
            self.expect_token(&Token::RParen)?;
            ColumnOption::Check(expr)
        } else {
            return self.expected("column option", self.peek_token());
        };

        Ok(ColumnOptionDef { name, option })
    }

    pub fn parse_referential_action(&mut self) -> Result<ReferentialAction, ParserError> {
        if self.parse_keyword(Keyword::RESTRICT) {
            Ok(ReferentialAction::Restrict)
        } else if self.parse_keyword(Keyword::CASCADE) {
            Ok(ReferentialAction::Cascade)
        } else if self.parse_keywords(&[Keyword::SET, Keyword::NULL]) {
            Ok(ReferentialAction::SetNull)
        } else if self.parse_keywords(&[Keyword::NO, Keyword::ACTION]) {
            Ok(ReferentialAction::NoAction)
        } else if self.parse_keywords(&[Keyword::SET, Keyword::DEFAULT]) {
            Ok(ReferentialAction::SetDefault)
        } else {
            self.expected(
                "one of RESTRICT, CASCADE, SET NULL, NO ACTION or SET DEFAULT",
                self.peek_token(),
            )
        }
    }
    
    pub fn parse_alter_drop_index(&mut self) -> Result<IndexDef, ParserError>{
        if self.parse_keyword(Keyword::INDEX) || self.parse_keyword(Keyword::KEY) {
            Ok(IndexDef::Normal(self.parse_alter_index_def_normal(false, false, true)?))
        }else if self.parse_keyword(Keyword::PRIMARY) {
            self.expect_keyword(Keyword::KEY)?;
            Ok(IndexDef::PrimaryKey(self.parse_alter_index_def_primary(true)?))
        }else if self.parse_keyword(Keyword::FOREIGN) {
            Ok(IndexDef::ForeignKey(self.parse_alter_index_def_normal(false, true, true)?))
        }else {
            self.expected(
                "alter table index def ",
                self.peek_token(),
            )
        }
    }

    pub fn parse_alter_add_index(&mut self) -> Result<AlterTableOperation, ParserError>{
        let constraint = self.parse_alter_index_constraint()?;
        let index_type = self.parse_alter_index_storge_type()?;
        let index = self.parse_alter_index_def()?;
        Ok(AlterTableOperation::AddIndex { index_def: IndexInfo{constraint, index_type, index} })
    }

    pub fn parse_alter_index_def(&mut self) -> Result<IndexDef, ParserError>{
        if self.parse_keyword(Keyword::INDEX) || self.parse_keyword(Keyword::KEY) {
            Ok(IndexDef::Normal(self.parse_alter_index_def_normal(false, false, false)?))
        }else if self.parse_keyword(Keyword::PRIMARY) {
            self.expect_keyword(Keyword::KEY)?;
            Ok(IndexDef::PrimaryKey(self.parse_alter_index_def_primary(false)?))
        }else if self.parse_keyword(Keyword::UNIQUE) {
            Ok(IndexDef::Unique(self.parse_alter_index_def_normal(true, false, false)?))
        }else if self.parse_keyword(Keyword::FOREIGN) {
            Ok(IndexDef::ForeignKey(self.parse_alter_index_def_normal(false, true, false)?))
        }else {
            self.expected(
                "alter table index def ",
                self.peek_token(),
            )
        }
    }


    pub fn parse_alter_index_def_primary(&mut self, drop: bool) -> Result<MysqlIndex, ParserError> {
        if drop{
            Ok(
                MysqlIndex{name:None, index_name:None, index_type: None, key_parts:None, index_option:None}
            )
        }else {
            let index_type = if self.parse_keyword(Keyword::USING){
                Some(self.parse_identifier()?)
            }else { None };
            let key_parts = Some(self.parse_parenthesized_column_list(Mandatory)?);
            let index_option = self.parse_alter_index_def_options()?;
            let (name, index_name) = (None, None);
            Ok(
                MysqlIndex{name, index_name, index_type, key_parts, index_option}
            )
        }
    }

    pub fn parse_alter_index_def_normal(&mut self, unique: bool, foreign: bool, drop: bool) -> Result<MysqlIndex, ParserError> {
        let name = if foreign{
            self.expect_keyword(Keyword::KEY)?;
            None
        }else {
            if !unique{
                self.prev_token();
            }
            Some(self.parse_identifier()?)
        };


        let index_name = if !self.consume_token(&Token::LParen){
            Some(self.parse_identifier()?)
        }else {
            self.prev_token();
            None
        };
        let (index_type, key_parts, index_option) = if drop{
            (None, None, None)
        }else {
            let index_type = if unique {
                if !self.consume_token(&Token::LParen){
                    Some(self.parse_identifier()?)
                }else {
                    self.prev_token();
                    None
                }
            } else {
                None
            };
            let key_parts = Some(self.parse_parenthesized_column_list(Mandatory)?);
            let index_option = self.parse_alter_index_def_options()?;
            (index_type, key_parts, index_option)
        };
        Ok(
            MysqlIndex{name, index_name, index_type, key_parts, index_option}
        )
    }

    pub fn parse_alter_index_def_options(&mut self) -> Result<Option<IndexOptions>, ParserError> {
        if self.consume_token(&Token::Comma) || self.consume_token(&Token::RParen) {
            self.prev_token();
            return Ok(None);
        }
        if self.consume_token(&Token::EOF) || self.consume_token(&Token::SemiColon){
            return Ok(None)
        }
        if self.parse_keyword(Keyword::KEY_BLOCK_SIZE){
            Ok(Some(IndexOptions::KeyBlockSize(self.parse_expr()?)))
        } else if self.parse_keyword(Keyword::WITH) {
            self.expect_keyword(Keyword::PARSER)?;
            Ok(Some(IndexOptions::WithParser(self.parse_identifier()?)))
        } else if self.parse_keyword(Keyword::USING) {
            Ok(Some(IndexOptions::IndexType(self.parse_identifier()?)))
        } else if self.parse_keyword(Keyword::COMMENT) {
            Ok(Some(IndexOptions::Comment(self.parse_expr()?)))
        }
        else if self.parse_keyword(Keyword::REFERENCES) {
            let table = self.parse_identifier()?;
            let column = self.parse_parenthesized_column_list(Mandatory)?;
            Ok(Some(IndexOptions::References {table, column}))
        } else {
            self.expected(
                "alter table for index options ",
                self.peek_token(),
            )
        }
    }

    pub fn parse_alter_index_constraint(&mut self) -> Result<Option<Ident>, ParserError>{
        return if self.parse_keyword(Keyword::CONSTRAINT) {
            Ok(Some(self.parse_identifier()?))
        } else {
            Ok(None)
        };
    }

    pub fn parse_alter_index_storge_type(&mut self) -> Result<Option<MysqlIndexStorageType>, ParserError>{
        return if self.parse_keyword(Keyword::FULLTEXT) {
            Ok(Some(MysqlIndexStorageType::FullText))
        } else if self.parse_keyword(Keyword::SPATIAL) {
            Ok(Some(MysqlIndexStorageType::Spatial))
        } else {
            Ok(None)
        }
    }

    pub fn parse_optional_table_constraint(
        &mut self,
    ) -> Result<Option<TableConstraint>, ParserError> {
        let name = if self.parse_keyword(Keyword::CONSTRAINT) {
            Some(self.parse_identifier()?)
        } else {
            None
        };
        match self.next_token() {
            Token::Word(w) if w.keyword == Keyword::PRIMARY || w.keyword == Keyword::UNIQUE => {
                let is_primary = w.keyword == Keyword::PRIMARY;
                if is_primary {
                    self.expect_keyword(Keyword::KEY)?;
                }
                let columns = self.parse_parenthesized_column_list(Mandatory)?;
                Ok(Some(TableConstraint::Unique {
                    name,
                    columns,
                    is_primary,
                }))
            }
            Token::Word(w) if w.keyword == Keyword::FOREIGN => {
                self.expect_keyword(Keyword::KEY)?;
                let columns = self.parse_parenthesized_column_list(Mandatory)?;
                self.expect_keyword(Keyword::REFERENCES)?;
                let foreign_table = self.parse_object_name()?;
                let referred_columns = self.parse_parenthesized_column_list(Mandatory)?;
                Ok(Some(TableConstraint::ForeignKey {
                    name,
                    columns,
                    foreign_table,
                    referred_columns,
                }))
            }
            Token::Word(w) if w.keyword == Keyword::CHECK => {
                self.expect_token(&Token::LParen)?;
                let expr = Box::new(self.parse_expr()?);
                self.expect_token(&Token::RParen)?;
                Ok(Some(TableConstraint::Check { name, expr }))
            }
            unexpected => {
                if name.is_some() {
                    self.expected("PRIMARY, UNIQUE, FOREIGN, or CHECK", unexpected)
                } else {
                    self.prev_token();
                    Ok(None)
                }
            }
        }
    }


    pub fn parse_with_options(&mut self) -> Result<Vec<SqlOption>, ParserError> {
        if self.parse_keyword(Keyword::WITH) {
            self.expect_token(&Token::LParen)?;
            let options = self.parse_comma_separated(Parser::parse_sql_option)?;
            self.expect_token(&Token::RParen)?;
            Ok(options)
        } else {
            Ok(vec![])
        }
    }

    pub fn parse_sql_option(&mut self) -> Result<SqlOption, ParserError> {
        let name = self.parse_identifier()?;
        self.expect_token(&Token::Eq)?;
        let value = self.parse_value()?;
        Ok(SqlOption { name, value })
    }

    pub fn parse_alter(&mut self) -> Result<Statement, ParserError> {
        self.expect_keyword(Keyword::TABLE)?;
        let _ = self.parse_keyword(Keyword::ONLY);
        let table_name = self.parse_object_name()?;
        let mut tmp = vec![];
        loop {
            if self.consume_token(&Token::EOF) || self.consume_token(&Token::SemiColon) {
                break
            }
            if self.consume_token(&Token::Comma){}

            let operation = if self.parse_keyword(Keyword::ADD) ||
                self.parse_keyword(Keyword::MODIFY){
                match self.dialect_type{
                    DBType::MySql=>{
                        if !self.parse_keyword(Keyword::COLUMN){
                            self.parse_alter_add_index()?
                        }else {
                            //let _ = self.parse_keyword(Keyword::COLUMN);
                            let column_def = self.parse_column_def()?;
                            AlterTableOperation::AddColumn { column_def }
                        }
                    }
                    _ => {
                        if let Some(constraint) = self.parse_optional_table_constraint()? {
                            AlterTableOperation::AddConstraint(constraint)
                        } else {
                            if !self.parse_keyword(Keyword::COLUMN){
                                self.parse_alter_add_index()?
                            }else {
                                //let _ = self.parse_keyword(Keyword::COLUMN);
                                let column_def = self.parse_column_def()?;
                                AlterTableOperation::AddColumn { column_def }
                            }

                        }
                    }
                }


            } else if self.parse_keyword(Keyword::CHANGE) {
                let _ = self.parse_keyword(Keyword::COLUMN);
                let old_column_name = self.parse_identifier()?;
                let new_column_def = self.parse_column_def()?;

                AlterTableOperation::ChangeColumn {
                    old_column_name,
                    new_column_def,
                }
            } else if self.parse_keyword(Keyword::RENAME) {
                if self.parse_keyword(Keyword::TO) {
                    let table_name = self.parse_identifier()?;
                    AlterTableOperation::RenameTable { table_name }
                } else {
                    let _ = self.parse_keyword(Keyword::COLUMN);
                    let old_column_name = self.parse_identifier()?;
                    self.expect_keyword(Keyword::TO)?;
                    let new_column_name = self.parse_identifier()?;
                    AlterTableOperation::RenameColumn {
                        old_column_name,
                        new_column_name,
                    }
                }
            } else if self.parse_keyword(Keyword::DROP) {
                if !self.parse_keyword(Keyword::COLUMN){
                    AlterTableOperation::DropIndex { index_def: self.parse_alter_drop_index()? }
                }else {
                    //let _ = self.parse_keyword(Keyword::COLUMN);
                    let if_exists = self.parse_keywords(&[Keyword::IF, Keyword::EXISTS]);
                    let column_name = self.parse_identifier()?;
                    let cascade = self.parse_keyword(Keyword::CASCADE);
                    AlterTableOperation::DropColumn {
                        column_name,
                        if_exists,
                        cascade,
                    }
                }

            } else {
                return self.expected("ADD, RENAME, or DROP after ALTER TABLE", self.peek_token());
            };

            tmp.push(operation)
        }

        Ok(Statement::AlterTable {
            name: table_name,
            operation: tmp,
        })
    }

    /// Parse a copy statement
    pub fn parse_copy(&mut self) -> Result<Statement, ParserError> {
        let table_name = self.parse_object_name()?;
        let columns = self.parse_parenthesized_column_list(Optional)?;
        self.expect_keywords(&[Keyword::FROM, Keyword::STDIN])?;
        self.expect_token(&Token::SemiColon)?;
        let values = self.parse_tsv()?;
        Ok(Statement::Copy {
            table_name,
            columns,
            values,
        })
    }

    /// Parse a tab separated values in
    /// COPY payload
    fn parse_tsv(&mut self) -> Result<Vec<Option<String>>, ParserError> {
        let values = self.parse_tab_value()?;
        Ok(values)
    }

    fn parse_tab_value(&mut self) -> Result<Vec<Option<String>>, ParserError> {
        let mut values = vec![];
        let mut content = String::from("");
        while let Some(t) = self.next_token_no_skip() {
            match t {
                Token::Whitespace(Whitespace::Tab) => {
                    values.push(Some(content.to_string()));
                    content.clear();
                }
                Token::Whitespace(Whitespace::Newline) => {
                    values.push(Some(content.to_string()));
                    content.clear();
                }
                Token::Backslash => {
                    if self.consume_token(&Token::Period) {
                        return Ok(values);
                    }
                    if let Token::Word(w) = self.next_token() {
                        if w.value == "N" {
                            values.push(None);
                        }
                    }
                }
                _ => {
                    content.push_str(&t.to_string());
                }
            }
        }
        Ok(values)
    }

    /// Parse a literal value (numbers, strings, date/time, booleans)
    fn parse_value(&mut self) -> Result<Value, ParserError> {
        match self.next_token() {
            Token::Word(w) => match w.keyword {
                Keyword::TRUE => Ok(Value::Boolean(true)),
                Keyword::FALSE => Ok(Value::Boolean(false)),
                Keyword::NULL => Ok(Value::Null),
                _ => self.expected("a concrete value", Token::Word(w)),
            },
            // The call to n.parse() returns a bigdecimal when the
            // bigdecimal feature is enabled, and is otherwise a no-op
            // (i.e., it returns the input string).
            Token::Number(ref n) => match n.parse() {
                Ok(n) => Ok(Value::Number(n)),
                Err(e) => parser_err!(format!("Could not parse '{}' as number: {}", n, e)),
            },
            Token::SingleQuotedString(ref s) => Ok(Value::SingleQuotedString(s.to_string())),
            Token::NationalStringLiteral(ref s) => Ok(Value::NationalStringLiteral(s.to_string())),
            Token::HexStringLiteral(ref s) => Ok(Value::HexStringLiteral(s.to_string())),
            Token::VariableString(ref v) => Ok(Value::VariableName(v.to_string())),
            Token::Char(ref c) => Ok(Value::Char(*c)),
            unexpected => self.expected("a value", unexpected),
        }
    }

    pub fn parse_number_value(&mut self) -> Result<Value, ParserError> {
        match self.parse_value()? {
            v @ Value::Number(_) => Ok(v),
            v @ Value::Char(_) => {
                if v == Value::Char('?'){
                    Ok(v)
                }else {
                    self.prev_token();
                    self.expected("literal number", self.peek_token())
                }
            }
            _ => {
                self.prev_token();
                self.expected("literal number", self.peek_token())
            }
        }
    }

    /// Parse an unsigned literal integer/long
    pub fn parse_literal_uint(&mut self) -> Result<u64, ParserError> {
        match self.next_token() {
            Token::Number(s) => s.parse::<u64>().map_err(|e| {
                ParserError::ParserError(format!("Could not parse '{}' as u64: {}", s, e))
            }),
            unexpected => self.expected("literal int", unexpected),
        }
    }

    /// Parse a literal string
    pub fn parse_literal_string(&mut self) -> Result<String, ParserError> {
        match self.next_token() {
            Token::SingleQuotedString(s) => Ok(s),
            unexpected => self.expected("literal string", unexpected),
        }
    }

    /// Parse a SQL datatype (in the context of a CREATE TABLE statement for example)
    pub fn parse_data_type(&mut self) -> Result<DataType, ParserError> {
        match self.next_token() {
            Token::Word(w) => match w.keyword {
                Keyword::BOOLEAN => Ok(DataType::Boolean),
                Keyword::FLOAT => Ok(DataType::Float(self.parse_optional_precision()?)),
                Keyword::REAL => Ok(DataType::Real),
                Keyword::DOUBLE => {
                    let _ = self.parse_keyword(Keyword::PRECISION);
                    Ok(DataType::Double)
                }
                Keyword::SMALLINT => {
                    let _ = self.parse_optional_precision()?;
                    Ok(DataType::SmallInt)
                },
                Keyword::INT | Keyword::INTEGER => {
                    let _ = self.parse_optional_precision()?;
                    Ok(DataType::Int)
                },
                Keyword::BIGINT => {
                    let _ = self.parse_optional_precision()?;
                    Ok(DataType::BigInt)
                },
                Keyword::VARCHAR => Ok(DataType::Varchar(self.parse_optional_precision()?)),
                Keyword::CHAR | Keyword::CHARACTER => {
                    if self.parse_keyword(Keyword::VARYING) {
                        Ok(DataType::Varchar(self.parse_optional_precision()?))
                    } else {
                        Ok(DataType::Char(self.parse_optional_precision()?))
                    }
                }
                Keyword::UUID => Ok(DataType::Uuid),
                Keyword::DATE => Ok(DataType::Date),
                Keyword::TIMESTAMP => {
                    // TBD: we throw away "with/without timezone" information
                    if self.parse_keyword(Keyword::WITH) || self.parse_keyword(Keyword::WITHOUT) {
                        self.expect_keywords(&[Keyword::TIME, Keyword::ZONE])?;
                    }
                    Ok(DataType::Timestamp)
                }
                Keyword::TIME => {
                    // TBD: we throw away "with/without timezone" information
                    if self.parse_keyword(Keyword::WITH) || self.parse_keyword(Keyword::WITHOUT) {
                        self.expect_keywords(&[Keyword::TIME, Keyword::ZONE])?;
                    }
                    Ok(DataType::Time)
                }
                // Interval types can be followed by a complicated interval
                // qualifier that we don't currently support. See
                // parse_interval_literal for a taste.
                Keyword::INTERVAL => Ok(DataType::Interval),
                Keyword::REGCLASS => Ok(DataType::Regclass),
                Keyword::TEXT => {
                    if self.consume_token(&Token::LBracket) {
                        // Note: this is postgresql-specific
                        self.expect_token(&Token::RBracket)?;
                        Ok(DataType::Array(Box::new(DataType::Text)))
                    } else {
                        Ok(DataType::Text)
                    }
                }
                Keyword::BYTEA => Ok(DataType::Bytea),
                Keyword::NUMERIC | Keyword::DECIMAL | Keyword::DEC => {
                    let (precision, scale) = self.parse_optional_precision_scale()?;
                    Ok(DataType::Decimal(precision, scale))
                }
                _ => {
                    self.prev_token();
                    let type_name = self.parse_object_name()?;
                    Ok(DataType::Custom(type_name))
                }
            },
            unexpected => self.expected("a data type name", unexpected),
        }
    }

    /// Parse `AS identifier` (or simply `identifier` if it's not a reserved keyword)
    /// Some examples with aliases: `SELECT 1 foo`, `SELECT COUNT(*) AS cnt`,
    /// `SELECT ... FROM t1 foo, t2 bar`, `SELECT ... FROM (...) AS bar`
    pub fn parse_optional_alias(
        &mut self,
        reserved_kwds: &[Keyword],
    ) -> Result<Option<Ident>, ParserError> {
        let after_as = self.parse_keyword(Keyword::AS);
        match self.next_token() {
            // Accept any identifier after `AS` (though many dialects have restrictions on
            // keywords that may appear here). If there's no `AS`: don't parse keywords,
            // which may start a construct allowed in this position, to be parsed as aliases.
            // (For example, in `FROM t1 JOIN` the `JOIN` will always be parsed as a keyword,
            // not an alias.)
            Token::Word(w) if after_as || !reserved_kwds.contains(&w.keyword) => {
                Ok(Some(w.to_ident()))
            }
            // MSSQL supports single-quoted strings as aliases for columns
            // We accept them as table aliases too, although MSSQL does not.
            //
            // Note, that this conflicts with an obscure rule from the SQL
            // standard, which we don't implement:
            // https://crate.io/docs/sql-99/en/latest/chapters/07.html#character-string-literal-s
            //    "[Obscure Rule] SQL allows you to break a long <character
            //    string literal> up into two or more smaller <character string
            //    literal>s, split by a <separator> that includes a newline
            //    character. When it sees such a <literal>, your DBMS will
            //    ignore the <separator> and treat the multiple strings as
            //    a single <literal>."
            Token::SingleQuotedString(s) => Ok(Some(Ident::with_quote('\'', s))),
            not_an_ident => {
                if after_as {
                    return self.expected("an identifier after AS", not_an_ident);
                }
                self.prev_token();
                Ok(None) // no alias found
            }
        }
    }

    /// Parse `AS identifier` when the AS is describing a table-valued object,
    /// like in `... FROM generate_series(1, 10) AS t (col)`. In this case
    /// the alias is allowed to optionally name the columns in the table, in
    /// addition to the table itself.
    pub fn parse_optional_table_alias(
        &mut self,
        reserved_kwds: &[Keyword],
    ) -> Result<Option<TableAlias>, ParserError> {
        match self.parse_optional_alias(reserved_kwds)? {
            Some(name) => {
                let columns = self.parse_parenthesized_column_list(Optional)?;
                Ok(Some(TableAlias { name, columns }))
            }
            None => Ok(None),
        }
    }

    /// Parse a possibly qualified, possibly quoted identifier, e.g.
    /// `foo` or `myschema."table"
    pub fn parse_object_name(&mut self) -> Result<ObjectName, ParserError> {
        let mut idents = vec![];
        loop {
            idents.push(self.parse_identifier()?);
            if !self.consume_token(&Token::Period) {
                break;
            }
        }
        Ok(ObjectName(idents))
    }

    /// Parse a simple one-word identifier (possibly quoted, possibly a keyword)
    pub fn parse_identifier(&mut self) -> Result<Ident, ParserError> {
        match self.next_token() {
            Token::Word(w) => Ok(w.to_ident()),
            Token::VariableString(v) => Ok(Ident{ value: v, quote_style: None }),
            unexpected => self.expected("identifier", unexpected),
        }
    }

    /// Parse a parenthesized comma-separated list of unqualified, possibly quoted identifiers
    pub fn parse_parenthesized_column_list(
        &mut self,
        optional: IsOptional,
    ) -> Result<Vec<Ident>, ParserError> {
        if self.consume_token(&Token::LParen) {
            let cols = self.parse_comma_separated(Parser::parse_identifier)?;
            self.expect_token(&Token::RParen)?;
            Ok(cols)
        } else if optional == Optional {
            Ok(vec![])
        } else {
            self.expected("a list of columns in parentheses", self.peek_token())
        }
    }

    pub fn parse_optional_precision(&mut self) -> Result<Option<u64>, ParserError> {
        if self.consume_token(&Token::LParen) {
            let n = self.parse_literal_uint()?;
            self.expect_token(&Token::RParen)?;
            Ok(Some(n))
        } else {
            Ok(None)
        }
    }

    pub fn parse_optional_precision_scale(
        &mut self,
    ) -> Result<(Option<u64>, Option<u64>), ParserError> {
        if self.consume_token(&Token::LParen) {
            let n = self.parse_literal_uint()?;
            let scale = if self.consume_token(&Token::Comma) {
                Some(self.parse_literal_uint()?)
            } else {
                None
            };
            self.expect_token(&Token::RParen)?;
            Ok((Some(n), scale))
        } else {
            Ok((None, None))
        }
    }

    pub fn parse_delete(&mut self) -> Result<Statement, ParserError> {
        self.expect_keyword(Keyword::FROM)?;
        let table_name = self.parse_object_name()?;
        let selection = if self.parse_keyword(Keyword::WHERE) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        Ok(Statement::Delete {
            table_name,
            selection,
        })
    }

    /// Parse a query expression, i.e. a `SELECT` statement optionally
    /// preceeded with some `WITH` CTE declarations and optionally followed
    /// by `ORDER BY`. Unlike some other parse_... methods, this one doesn't
    /// expect the initial keyword to be already consumed
    pub fn parse_query(&mut self) -> Result<Query, ParserError> {
        let ctes = if self.parse_keyword(Keyword::WITH) {
            // TODO: optional RECURSIVE
            self.parse_comma_separated(Parser::parse_cte)?
        } else {
            vec![]
        };
        let body = self.parse_query_body(0)?;

        let order_by = if self.parse_keywords(&[Keyword::ORDER, Keyword::BY]) {
            self.parse_comma_separated(Parser::parse_order_by_expr)?
        } else {
            vec![]
        };

        let (limit, offset) = if self.parse_keyword(Keyword::LIMIT) {
            self.parse_mysql_limit()?
        } else {
            (None,None)
        };

        let update = if self.parse_keyword(Keyword::FOR){
            self.expect_keyword(Keyword::UPDATE)?;
            true
        }else {
            false
        };
        // let offset = if self.parse_keyword(Keyword::OFFSET) {
        //     Some(self.parse_offset()?)
        // } else {
        //     None
        // };


        let fetch = if self.parse_keyword(Keyword::FETCH) {
            Some(self.parse_fetch()?)
        } else {
            None
        };

        Ok(Query {
            ctes,
            body,
            limit,
            order_by,
            offset,
            update,
            fetch,
        })
    }

    /// Parse a CTE (`alias [( col1, col2, ... )] AS (subquery)`)
    fn parse_cte(&mut self) -> Result<Cte, ParserError> {
        let alias = TableAlias {
            name: self.parse_identifier()?,
            columns: self.parse_parenthesized_column_list(Optional)?,
        };
        self.expect_keyword(Keyword::AS)?;
        self.expect_token(&Token::LParen)?;
        let query = self.parse_query()?;
        self.expect_token(&Token::RParen)?;
        Ok(Cte { alias, query })
    }

    /// Parse a "query body", which is an expression with roughly the
    /// following grammar:
    /// ```text
    ///   query_body ::= restricted_select | '(' subquery ')' | set_operation
    ///   restricted_select ::= 'SELECT' [expr_list] [ from ] [ where ] [ groupby_having ]
    ///   subquery ::= query_body [ order_by_limit ]
    ///   set_operation ::= query_body { 'UNION' | 'EXCEPT' | 'INTERSECT' } [ 'ALL' ] query_body
    /// ```
    fn parse_query_body(&mut self, precedence: u8) -> Result<SetExpr, ParserError> {
        // We parse the expression using a Pratt parser, as in `parse_expr()`.
        // Start by parsing a restricted SELECT or a `(subquery)`:
        let mut expr = if self.parse_keyword(Keyword::SELECT) {
            SetExpr::Select(Box::new(self.parse_select()?))
        } else if self.consume_token(&Token::LParen) {
            // CTEs are not allowed here, but the parser currently accepts them
            let subquery = self.parse_query()?;
            self.expect_token(&Token::RParen)?;
            SetExpr::Query(Box::new(subquery))
        } else if self.parse_keyword(Keyword::VALUES)  {
            SetExpr::Values(self.parse_values()?)
        } else if self.parse_keyword(Keyword::VALUE) {
            SetExpr::Value(self.parse_values()?)
        }
        else {
            return self.expected(
                "SELECT, VALUES, or a subquery in the query body",
                self.peek_token(),
            );
        };
        loop {
            // The query can be optionally followed by a set operator:
            let op = self.parse_set_operator(&self.peek_token());
            let next_precedence = match op {
                // UNION and EXCEPT have the same binding power and evaluate left-to-right
                Some(SetOperator::Union) | Some(SetOperator::Except) => 10,
                // INTERSECT has higher precedence than UNION/EXCEPT
                Some(SetOperator::Intersect) => 20,
                // Unexpected token or EOF => stop parsing the query body
                None => break,
            };
            if precedence >= next_precedence {
                break;
            }
            self.next_token(); // skip past the set operator
            expr = SetExpr::SetOperation {
                left: Box::new(expr),
                op: op.unwrap(),
                all: self.parse_keyword(Keyword::ALL),
                right: Box::new(self.parse_query_body(next_precedence)?),
            };
        }

        Ok(expr)
    }

    fn parse_on_duplicate_key_update(&mut self) -> Result<bool, ParserError> {
        if self.parse_keyword(Keyword::ON){
            if self.parse_keyword(Keyword::DUPLICATE){
                if self.parse_keyword(Keyword::KEY){
                    if self.parse_keyword(Keyword::UPDATE){
                        return Ok(true);
                    }else {
                        self.expected("on duplicate key update error", self.peek_token())
                    }
                }else {
                    self.expected("on duplicate key update error", self.peek_token())
                }
            }else {
                self.expected("on duplicate key update error", self.peek_token())
            }
        }else {
            return Ok(false)
        }
    }

    fn parse_set_operator(&mut self, token: &Token) -> Option<SetOperator> {
        match token {
            Token::Word(w) if w.keyword == Keyword::UNION => Some(SetOperator::Union),
            Token::Word(w) if w.keyword == Keyword::EXCEPT => Some(SetOperator::Except),
            Token::Word(w) if w.keyword == Keyword::INTERSECT => Some(SetOperator::Intersect),
            _ => None,
        }
    }

    /// Parse a restricted `SELECT` statement (no CTEs / `UNION` / `ORDER BY`),
    /// assuming the initial `SELECT` was already consumed
    pub fn parse_select(&mut self) -> Result<Select, ParserError> {
        let comment = self.parse_comment_for_select()?;
        let distinct = self.parse_all_or_distinct()?;

        let top = if self.parse_keyword(Keyword::TOP) {
            Some(self.parse_top()?)
        } else {
            None
        };

        // println!("123");
        let projection = self.parse_comma_separated(Parser::parse_select_item)?;
        // println!("aaa");
        // Note that for keywords to be properly handled here, they need to be
        // added to `RESERVED_FOR_COLUMN_ALIAS` / `RESERVED_FOR_TABLE_ALIAS`,
        // otherwise they may be parsed as an alias as part of the `projection`
        // or `from`.

        let from = if self.parse_keyword(Keyword::FROM) {
            self.parse_comma_separated(Parser::parse_table_and_joins)?
        } else {
            vec![]
        };

        let selection = if self.parse_keyword(Keyword::WHERE) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        let group_by = if self.parse_keywords(&[Keyword::GROUP, Keyword::BY]) {
            self.parse_comma_separated(Parser::parse_expr)?
        } else {
            vec![]
        };

        let having = if self.parse_keyword(Keyword::HAVING) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        Ok(Select {
            comment,
            distinct,
            top,
            projection,
            from,
            selection,
            group_by,
            having,
        })
    }

    fn parse_force_for_select(&mut self) -> Result<Ident, ParserError>{
        if self.parse_keyword(Keyword::INDEX){
            self.expect_token(&Token::LParen)?;
            let force = self.parse_identifier()?;
            self.expect_token(&Token::RParen)?;
            Ok(force)
        }else {
            self.expected("an error in your SQL syntax,force index {}", self.peek_token())
        }

    }

    fn parse_comment_for_select(&mut self) -> Result<Option<Ident>, ParserError>{
        match self.next_token_no_ignore_comment(){
            Token::Whitespace(Whitespace::MultiLineComment(v)) => {
                Ok(Some(Ident{ value: v.clone().replace(" ",""), quote_style: None }))
            }
            _ => {
                self.prev_token();
                Ok(None)
            }
        }
    }

    pub fn parse_reload(&mut self) -> Result<Statement, ParserError>{
        return match self.next_token() {
            Token::Word(w) => match w.keyword {
                Keyword::CONFIG => {
                    self.prev_token();
                    let variable = self.parse_identifier()?;
                    if self.consume_token(&Token::EOF) {
                        Ok(Statement::ReLoad {
                            variable,
                            selection: None
                        })
                    } else {
                        self.expected("reload config does not support parameters", self.peek_token())
                    }
                }
                Keyword::USER => {
                    self.prev_token();
                    let variable = self.parse_identifier()?;
                    if !self.consume_token(&Token::EOF) {
                        let selection = if self.parse_keyword(Keyword::WHERE) {
                            Some(self.parse_expr()?)
                        } else {
                            None
                        };
                        Ok(Statement::ReLoad {
                            variable,
                            selection
                        })
                    } else {
                        Ok(Statement::ReLoad {
                            variable,
                            selection: None
                        })
                    }
                }
                _ => {
                    self.prev_token();
                    self.expected("reload only support config and user privileges", self.peek_token())
                }
            },
            _ => {
                self.prev_token();
                self.expected("reload only support config and user privileges", self.peek_token())
            }
        }
    }

    pub fn parse_set(&mut self) -> Result<Statement, ParserError> {
        let modifier = self.parse_one_of_keywords(&[Keyword::SESSION, Keyword::LOCAL]);
        let variable = self.parse_identifier()?;
        if self.consume_token(&Token::Eq) || self.parse_keyword(Keyword::TO) {
            let value = self.parse_set_variables_value()?;
            if !self.consume_token(&Token::EOF){
                let selection = if self.parse_keyword(Keyword::WHERE) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                Ok(Statement::AdminSetVariable {
                    variable,
                    value,
                    selection
                })
            }else {
                Ok(Statement::SetVariable {
                    local: modifier == Some(Keyword::LOCAL),
                    variable,
                    value,
                })
            }
        } else if variable.value == "TRANSACTION" && modifier.is_none() {
            Ok(Statement::SetTransaction {
                modes: self.parse_transaction_modes()?,
            })
        } else if variable.value.to_lowercase() == "names" && modifier.is_none() {
            Ok(Statement::SetVariable {
                local: modifier == Some(Keyword::LOCAL),
                variable,
                value: self.parse_set_variables_value()?,
            })
        } else {
            self.expected("equals sign or TO", self.peek_token())
        }
    }

    fn parse_set_variables_value(&mut self) -> Result<SetVariableValue, ParserError>{
        let token = self.peek_token();
        let value = match (self.parse_value(), token) {
            (Ok(value), _) => SetVariableValue::Literal(value),
            (Err(_), Token::Word(ident)) => SetVariableValue::Ident(ident.to_ident()),
            (Err(_), unexpected) => self.expected("variable value", unexpected)?,
        };
        Ok(value)
    }

    pub fn parse_show(&mut self) -> Result<Statement, ParserError> {
        if self
            .parse_one_of_keywords(&[
                Keyword::EXTENDED,
                Keyword::FULL,
                Keyword::COLUMNS,
                Keyword::FIELDS,
            ])
            .is_some()
        {
            self.prev_token();
            self.parse_show_columns()
        } else if self.parse_keyword(Keyword::CREATE) {
            if self.parse_keyword(Keyword::TABLE){
                let table_name = self.parse_object_name()?;
                return Ok(Statement::ShowCreate { table_name });
            }else {
                self.prev_token();
                return self.expected("equals sign or TO", self.peek_token());
            }
        }else {
            let global = if self.parse_keyword(Keyword::GLOBAL){
                true
            }else {
                false
            };
            let variable = self.parse_identifier()?;
            let selection = if self.parse_keyword(Keyword::WHERE) {
                Some(self.parse_expr()?)
            } else {
                None
            };

            Ok(Statement::ShowVariable {
                variable,
                global,
                selection
            })
        }
    }

    fn parse_show_columns(&mut self) -> Result<Statement, ParserError> {
        let extended = self.parse_keyword(Keyword::EXTENDED);
        let full = self.parse_keyword(Keyword::FULL);
        self.expect_one_of_keywords(&[Keyword::COLUMNS, Keyword::FIELDS])?;
        self.expect_one_of_keywords(&[Keyword::FROM, Keyword::IN])?;
        let table_name = self.parse_object_name()?;
        // MySQL also supports FROM <database> here. In other words, MySQL
        // allows both FROM <table> FROM <database> and FROM <database>.<table>,
        // while we only support the latter for now.
        let filter = self.parse_show_statement_filter()?;
        Ok(Statement::ShowColumns {
            extended,
            full,
            table_name,
            filter,
        })
    }

    fn parse_show_statement_filter(&mut self) -> Result<Option<ShowStatementFilter>, ParserError> {
        if self.parse_keyword(Keyword::LIKE) {
            Ok(Some(ShowStatementFilter::Like(
                self.parse_literal_string()?,
            )))
        } else if self.parse_keyword(Keyword::WHERE) {
            Ok(Some(ShowStatementFilter::Where(self.parse_expr()?)))
        } else {
            Ok(None)
        }
    }

    pub fn parse_table_and_joins(&mut self) -> Result<TableWithJoins, ParserError> {
        let relation = self.parse_table_factor()?;

        // Note that for keywords to be properly handled here, they need to be
        // added to `RESERVED_FOR_TABLE_ALIAS`, otherwise they may be parsed as
        // a table alias.
        let mut joins = vec![];
        loop {
            // if self.parse_keyword(Keyword::FORCE){
            //     self.prev_token();
            //     break;
            // }
            let join = if self.parse_keyword(Keyword::CROSS) {
                let join_operator = if self.parse_keyword(Keyword::JOIN) {
                    JoinOperator::CrossJoin
                } else if self.parse_keyword(Keyword::APPLY) {
                    // MSSQL extension, similar to CROSS JOIN LATERAL
                    JoinOperator::CrossApply
                } else {
                    return self.expected("JOIN or APPLY after CROSS", self.peek_token());
                };
                Join {
                    relation: self.parse_table_factor()?,
                    join_operator,
                }
            } else if self.parse_keyword(Keyword::OUTER) {
                // MSSQL extension, similar to LEFT JOIN LATERAL .. ON 1=1
                self.expect_keyword(Keyword::APPLY)?;
                Join {
                    relation: self.parse_table_factor()?,
                    join_operator: JoinOperator::OuterApply,
                }
            } else {
                let natural = self.parse_keyword(Keyword::NATURAL);
                let peek_keyword = if let Token::Word(w) = self.peek_token() {
                    w.keyword
                } else {
                    Keyword::NoKeyword
                };

                let join_operator_type = match peek_keyword {
                    Keyword::INNER | Keyword::JOIN => {
                        let _ = self.parse_keyword(Keyword::INNER);
                        self.expect_keyword(Keyword::JOIN)?;
                        JoinOperator::Inner
                    }
                    kw @ Keyword::LEFT | kw @ Keyword::RIGHT | kw @ Keyword::FULL => {
                        let _ = self.next_token();
                        let _ = self.parse_keyword(Keyword::OUTER);
                        self.expect_keyword(Keyword::JOIN)?;
                        match kw {
                            Keyword::LEFT => JoinOperator::LeftOuter,
                            Keyword::RIGHT => JoinOperator::RightOuter,
                            Keyword::FULL => JoinOperator::FullOuter,
                            _ => unreachable!(),
                        }
                    }
                    Keyword::OUTER => {
                        return self.expected("LEFT, RIGHT, or FULL", self.peek_token())
                    }
                    _ if natural => {
                        return self.expected("a join type after NATURAL", self.peek_token());
                    }
                    _ => break,
                };
                let relation = self.parse_table_factor()?;
                let join_constraint = self.parse_join_constraint(natural)?;
                Join {
                    relation,
                    join_operator: join_operator_type(join_constraint),
                }
            };
            joins.push(join);
        }
        Ok(TableWithJoins { relation, joins })
    }

    /// A table name or a parenthesized subquery, followed by optional `[AS] alias`
    pub fn parse_table_factor(&mut self) -> Result<TableFactor, ParserError> {
        if self.parse_keyword(Keyword::LATERAL) {
            // LATERAL must always be followed by a subquery.
            if !self.consume_token(&Token::LParen) {
                self.expected("subquery after LATERAL", self.peek_token())?;
            }
            return self.parse_derived_table_factor(Lateral);
        }

        if self.consume_token(&Token::LParen) {
            // A left paren introduces either a derived table (i.e., a subquery)
            // or a nested join. It's nearly impossible to determine ahead of
            // time which it is... so we just try to parse both.
            //
            // Here's an example that demonstrates the complexity:
            //                     /-------------------------------------------------------\
            //                     | /-----------------------------------\                 |
            //     SELECT * FROM ( ( ( (SELECT 1) UNION (SELECT 2) ) AS t1 NATURAL JOIN t2 ) )
            //                   ^ ^ ^ ^
            //                   | | | |
            //                   | | | |
            //                   | | | (4) belongs to a SetExpr::Query inside the subquery
            //                   | | (3) starts a derived table (subquery)
            //                   | (2) starts a nested join
            //                   (1) an additional set of parens around a nested join
            //

            // If the recently consumed '(' starts a derived table, the call to
            // `parse_derived_table_factor` below will return success after parsing the
            // subquery, followed by the closing ')', and the alias of the derived table.
            // In the example above this is case (3).
            return_ok_if_some!(
                self.maybe_parse(|parser| parser.parse_derived_table_factor(NotLateral))
            );
            // A parsing error from `parse_derived_table_factor` indicates that the '(' we've
            // recently consumed does not start a derived table (cases 1, 2, or 4).
            // `maybe_parse` will ignore such an error and rewind to be after the opening '('.

            // Inside the parentheses we expect to find a table factor
            // followed by some joins or another level of nesting.
            let table_and_joins = self.parse_table_and_joins()?;
            self.expect_token(&Token::RParen)?;
            // The SQL spec prohibits derived and bare tables from appearing
            // alone in parentheses. We don't enforce this as some databases
            // (e.g. Snowflake) allow such syntax.
            Ok(TableFactor::NestedJoin(Box::new(table_and_joins)))
        } else {
            let name = self.parse_object_name()?;
            // Postgres, MSSQL: table-valued functions:
            let args = if self.consume_token(&Token::LParen) {
                self.parse_optional_args()?
            } else {
                vec![]
            };
            // mysql force index
            let mut force = None;
            if self.parse_keyword(Keyword::FORCE){
                force = Some(self.parse_force_for_select()?);
            }
            let alias = self.parse_optional_table_alias(keywords::RESERVED_FOR_TABLE_ALIAS)?;

            // mysql force index agin
            if self.parse_keyword(Keyword::FORCE){
                force = Some(self.parse_force_for_select()?);
            }
            // MSSQL-specific table hints:
            let mut with_hints = vec![];
            if self.parse_keyword(Keyword::WITH) {
                if self.consume_token(&Token::LParen) {
                    with_hints = self.parse_comma_separated(Parser::parse_expr)?;
                    self.expect_token(&Token::RParen)?;
                } else {
                    // rewind, as WITH may belong to the next statement's CTE
                    self.prev_token();
                }
            };
            Ok(TableFactor::Table {
                name,
                alias,
                force,
                args,
                with_hints,
            })
        }
    }

    pub fn parse_derived_table_factor(
        &mut self,
        lateral: IsLateral,
    ) -> Result<TableFactor, ParserError> {
        let subquery = Box::new(self.parse_query()?);
        self.expect_token(&Token::RParen)?;
        let alias = self.parse_optional_table_alias(keywords::RESERVED_FOR_TABLE_ALIAS)?;
        Ok(TableFactor::Derived {
            lateral: match lateral {
                Lateral => true,
                NotLateral => false,
            },
            subquery,
            alias,
        })
    }

    fn parse_join_constraint(&mut self, natural: bool) -> Result<JoinConstraint, ParserError> {
        if natural {
            Ok(JoinConstraint::Natural)
        } else if self.parse_keyword(Keyword::ON) {
            let constraint = self.parse_expr()?;
            Ok(JoinConstraint::On(constraint))
        } else if self.parse_keyword(Keyword::USING) {
            let columns = self.parse_parenthesized_column_list(Mandatory)?;
            Ok(JoinConstraint::Using(columns))
        } else {
            self.expected("ON, or USING after JOIN", self.peek_token())
        }
    }

    /// Parse an INSERT statement
    pub fn parse_insert(&mut self) -> Result<Statement, ParserError> {
        let mut priority = None;
        let mut ignore = false;
        if self.parse_keyword(Keyword::LOW_PRIORITY){
            priority = Some(Priority::LOW_PRIORITY);
        }else if self.parse_keyword(Keyword::HIGH_PRIORITY) {
            priority = Some(Priority::HIGH_PRIORITY);
        }else if self.parse_keyword(Keyword::DELAYED){
            priority = Some(Priority::DELAYED);
        }

        if self.parse_keyword(Keyword::IGNORE){
            ignore = true;
        }

        if let Err(e) = self.expect_keyword(Keyword::INTO){}
        let table_name = self.parse_object_name()?;
        let columns = self.parse_parenthesized_column_list(Optional)?;
        let source = Box::new(self.parse_query()?);
        let update = if self.parse_on_duplicate_key_update()? {
            Some(self.parse_comma_separated(Parser::parse_assignment)?)
        }else {
            None
        };
        Ok(Statement::Insert {
            priority,
            ignore,
            table_name,
            columns,
            source,
            update
        })
    }

    pub fn parse_update(&mut self) -> Result<Statement, ParserError> {
        let table_name = self.parse_object_name()?;
        self.expect_keyword(Keyword::SET)?;
        let assignments = self.parse_comma_separated(Parser::parse_assignment)?;
        let selection = if self.parse_keyword(Keyword::WHERE) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        let (limit, _) = if self.parse_keyword(Keyword::LIMIT) {
            self.parse_mysql_limit()?
        } else {
            (None,None)
        };

        Ok(Statement::Update {
            table_name,
            assignments,
            selection,
            limit
        })
    }

    /// Parse a `var = expr` assignment, used in an UPDATE statement
    pub fn parse_assignment(&mut self) -> Result<Assignment, ParserError> {
        let id = self.parse_identifier()?;
        self.expect_token(&Token::Eq)?;
        let value = self.parse_expr()?;
        Ok(Assignment { id, value })
    }

    pub fn parse_optional_args(&mut self) -> Result<Vec<Expr>, ParserError> {
        if self.consume_token(&Token::RParen) {
            Ok(vec![])
        } else {
            let args = self.parse_comma_separated(Parser::parse_expr)?;
            self.expect_token(&Token::RParen)?;
            Ok(args)
        }
    }

    /// Parse a comma-delimited list of projections after SELECT
    pub fn parse_select_item(&mut self) -> Result<SelectItem, ParserError> {
        let expr = self.parse_expr()?;
        if let Expr::Wildcard = expr {
            Ok(SelectItem::Wildcard)
        } else if let Expr::QualifiedWildcard(prefix) = expr {
            Ok(SelectItem::QualifiedWildcard(ObjectName(prefix)))
        } else {
            // `expr` is a regular SQL expression and can be followed by an alias
            if let Some(alias) = self.parse_optional_alias(keywords::RESERVED_FOR_COLUMN_ALIAS)? {
                Ok(SelectItem::ExprWithAlias { expr, alias })
            } else {
                Ok(SelectItem::UnnamedExpr(expr))
            }
        }
    }

    /// Parse an expression, optionally followed by ASC or DESC (used in ORDER BY)
    pub fn parse_order_by_expr(&mut self) -> Result<OrderByExpr, ParserError> {
        let expr = self.parse_expr()?;

        let asc = if self.parse_keyword(Keyword::ASC) {
            Some(true)
        } else if self.parse_keyword(Keyword::DESC) {
            Some(false)
        } else {
            None
        };

        let nulls_first = if self.parse_keywords(&[Keyword::NULLS, Keyword::FIRST]) {
            Some(true)
        } else if self.parse_keywords(&[Keyword::NULLS, Keyword::LAST]) {
            Some(false)
        } else {
            None
        };

        Ok(OrderByExpr {
            expr,
            asc,
            nulls_first,
        })
    }

    /// Parse a TOP clause, MSSQL equivalent of LIMIT,
    /// that follows after SELECT [DISTINCT].
    pub fn parse_top(&mut self) -> Result<Top, ParserError> {
        let quantity = if self.consume_token(&Token::LParen) {
            let quantity = self.parse_expr()?;
            self.expect_token(&Token::RParen)?;
            Some(quantity)
        } else {
            Some(Expr::Value(self.parse_number_value()?))
        };

        let percent = self.parse_keyword(Keyword::PERCENT);

        let with_ties = self.parse_keywords(&[Keyword::WITH, Keyword::TIES]);

        Ok(Top {
            with_ties,
            percent,
            quantity,
        })
    }

    pub fn parse_mysql_limit(&mut self) -> Result<(Option<Expr>, Option<Offset>), ParserError> {
        if self.parse_keyword(Keyword::ALL) {
            Ok((None, None))
        } else {
            let limit_value = Some(Expr::Value(self.parse_number_value()?));
            if self.parse_keyword(Keyword::OFFSET){
                Ok((limit_value,Some(self.parse_offset()?)))
            }else if self.peek_token() == Token::Comma {
                self.next_token();
                Ok((limit_value,Some(self.parse_offset()?)))
            }
            else {
                Ok((limit_value, None))
            }
        }
    }

    /// Parse a LIMIT clause
    pub fn parse_limit(&mut self) -> Result<Option<Expr>, ParserError> {
        if self.parse_keyword(Keyword::ALL) {
            Ok(None)
        } else {
            Ok(Some(Expr::Value(self.parse_number_value()?)))
        }
    }

    /// Parse an OFFSET clause
    pub fn parse_offset(&mut self) -> Result<Offset, ParserError> {
        let value = Expr::Value(self.parse_number_value()?);
        let rows = if self.parse_keyword(Keyword::ROW) {
            OffsetRows::Row
        } else if self.parse_keyword(Keyword::ROWS) {
            OffsetRows::Rows
        } else {
            OffsetRows::None
        };
        Ok(Offset { value, rows })
    }

    /// Parse a FETCH clause
    pub fn parse_fetch(&mut self) -> Result<Fetch, ParserError> {
        self.expect_one_of_keywords(&[Keyword::FIRST, Keyword::NEXT])?;
        let (quantity, percent) = if self
            .parse_one_of_keywords(&[Keyword::ROW, Keyword::ROWS])
            .is_some()
        {
            (None, false)
        } else {
            let quantity = Expr::Value(self.parse_value()?);
            let percent = self.parse_keyword(Keyword::PERCENT);
            self.expect_one_of_keywords(&[Keyword::ROW, Keyword::ROWS])?;
            (Some(quantity), percent)
        };
        let with_ties = if self.parse_keyword(Keyword::ONLY) {
            false
        } else if self.parse_keywords(&[Keyword::WITH, Keyword::TIES]) {
            true
        } else {
            return self.expected("one of ONLY or WITH TIES", self.peek_token());
        };
        Ok(Fetch {
            with_ties,
            percent,
            quantity,
        })
    }

    pub fn parse_values(&mut self) -> Result<Values, ParserError> {
        let values = self.parse_comma_separated(|parser| {
            parser.expect_token(&Token::LParen)?;
            let exprs = parser.parse_comma_separated(Parser::parse_expr)?;
            parser.expect_token(&Token::RParen)?;
            Ok(exprs)
        })?;
        Ok(Values(values))
    }


    pub fn parse_start_transaction(&mut self) -> Result<Statement, ParserError> {
        self.expect_keyword(Keyword::TRANSACTION)?;
        Ok(Statement::StartTransaction {
            modes: self.parse_transaction_modes()?,
        })
    }

    pub fn parse_begin(&mut self) -> Result<Statement, ParserError> {
        let _ = self.parse_one_of_keywords(&[Keyword::TRANSACTION, Keyword::WORK]);
        Ok(Statement::StartTransaction {
            modes: self.parse_transaction_modes()?,
        })
    }

    pub fn parse_transaction_modes(&mut self) -> Result<Vec<TransactionMode>, ParserError> {
        let mut modes = vec![];
        let mut required = false;
        loop {
            let mode = if self.parse_keywords(&[Keyword::ISOLATION, Keyword::LEVEL]) {
                let iso_level = if self.parse_keywords(&[Keyword::READ, Keyword::UNCOMMITTED]) {
                    TransactionIsolationLevel::ReadUncommitted
                } else if self.parse_keywords(&[Keyword::READ, Keyword::COMMITTED]) {
                    TransactionIsolationLevel::ReadCommitted
                } else if self.parse_keywords(&[Keyword::REPEATABLE, Keyword::READ]) {
                    TransactionIsolationLevel::RepeatableRead
                } else if self.parse_keyword(Keyword::SERIALIZABLE) {
                    TransactionIsolationLevel::Serializable
                } else {
                    self.expected("isolation level", self.peek_token())?
                };
                TransactionMode::IsolationLevel(iso_level)
            } else if self.parse_keywords(&[Keyword::READ, Keyword::ONLY]) {
                TransactionMode::AccessMode(TransactionAccessMode::ReadOnly)
            } else if self.parse_keywords(&[Keyword::READ, Keyword::WRITE]) {
                TransactionMode::AccessMode(TransactionAccessMode::ReadWrite)
            } else if required {
                self.expected("transaction mode", self.peek_token())?
            } else {
                break;
            };
            modes.push(mode);
            // ANSI requires a comma after each transaction mode, but
            // PostgreSQL, for historical reasons, does not. We follow
            // PostgreSQL in making the comma optional, since that is strictly
            // more general.
            required = self.consume_token(&Token::Comma);
        }
        Ok(modes)
    }

    pub fn parse_commit(&mut self) -> Result<Statement, ParserError> {
        Ok(Statement::Commit {
            chain: self.parse_commit_rollback_chain()?,
        })
    }

    pub fn parse_rollback(&mut self) -> Result<Statement, ParserError> {
        Ok(Statement::Rollback {
            chain: self.parse_commit_rollback_chain()?,
        })
    }

    pub fn parse_commit_rollback_chain(&mut self) -> Result<bool, ParserError> {
        let _ = self.parse_one_of_keywords(&[Keyword::TRANSACTION, Keyword::WORK]);
        if self.parse_keyword(Keyword::AND) {
            let chain = !self.parse_keyword(Keyword::NO);
            self.expect_keyword(Keyword::CHAIN)?;
            Ok(chain)
        } else {
            Ok(false)
        }
    }
}

impl Word {
    pub fn to_ident(&self) -> Ident {
        Ident {
            value: self.value.clone(),
            quote_style: self.quote_style,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::all_dialects;

    #[test]
    fn test_prev_index() {
        let sql = "SELECT version";
        all_dialects().run_parser_method(sql, |parser| {
            assert_eq!(parser.peek_token(), Token::make_keyword("SELECT"));
            assert_eq!(parser.next_token(), Token::make_keyword("SELECT"));
            parser.prev_token();
            assert_eq!(parser.next_token(), Token::make_keyword("SELECT"));
            assert_eq!(parser.next_token(), Token::make_word("version", None));
            parser.prev_token();
            assert_eq!(parser.peek_token(), Token::make_word("version", None));
            assert_eq!(parser.next_token(), Token::make_word("version", None));
            assert_eq!(parser.peek_token(), Token::EOF);
            parser.prev_token();
            assert_eq!(parser.next_token(), Token::make_word("version", None));
            assert_eq!(parser.next_token(), Token::EOF);
            assert_eq!(parser.next_token(), Token::EOF);
            parser.prev_token();
        });
    }
}

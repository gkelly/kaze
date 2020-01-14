use super::il::*;
use super::stack::*;

use crate::module;

use std::collections::HashMap;

pub type InstanceStack<'a> = Stack<*const module::Instance<'a>>;

#[derive(Clone)]
pub struct RegNames {
    pub value_name: String,
    pub next_name: String,
}

pub struct Compiler<'a> {
    pub reg_names: HashMap<(InstanceStack<'a>, *const module::Signal<'a>), RegNames>,
    signal_exprs: HashMap<(InstanceStack<'a>, *const module::Signal<'a>), Expr>,

    pub prop_assignments: Vec<Assignment>,

    local_count: u32,
}

impl<'a> Compiler<'a> {
    pub fn new() -> Compiler<'a> {
        Compiler {
            reg_names: HashMap::new(),
            signal_exprs: HashMap::new(),

            prop_assignments: Vec::new(),

            local_count: 0,
        }
    }

    pub fn gather_regs(
        &mut self,
        signal: &'a module::Signal<'a>,
        instance_stack: &InstanceStack<'a>,
    ) {
        match signal.data {
            module::SignalData::Lit { .. } => (),

            module::SignalData::Input { ref name, .. } => {
                if let Some((instance, instance_stack_tail)) = instance_stack.pop() {
                    let instance = unsafe { &*instance };
                    // TODO: Report error if input isn't driven
                    //  Should we report errors for all undriven inputs here?
                    self.gather_regs(instance.driven_inputs.borrow()[name], &instance_stack_tail);
                }
            }

            module::SignalData::Reg { ref next, .. } => {
                let key = (instance_stack.clone(), signal as *const _);
                if self.reg_names.contains_key(&key) {
                    return;
                }
                let value_name = format!("__reg{}", self.reg_names.len());
                let next_name = format!("{}_next", value_name);
                self.reg_names.insert(
                    key,
                    RegNames {
                        value_name,
                        next_name,
                    },
                );
                // TODO: Proper error and test(s)
                self.gather_regs(
                    next.borrow().expect("Discovered undriven register(s)"),
                    instance_stack,
                );
            }

            module::SignalData::UnOp { source, .. } => {
                self.gather_regs(source, instance_stack);
            }
            module::SignalData::BinOp { lhs, rhs, .. } => {
                self.gather_regs(lhs, instance_stack);
                self.gather_regs(rhs, instance_stack);
            }

            module::SignalData::Bits { source, .. } => {
                self.gather_regs(source, instance_stack);
            }

            module::SignalData::Repeat { source, .. } => {
                self.gather_regs(source, instance_stack);
            }
            module::SignalData::Concat { lhs, rhs } => {
                self.gather_regs(lhs, instance_stack);
                self.gather_regs(rhs, instance_stack);
            }

            module::SignalData::Mux { a, b, sel } => {
                self.gather_regs(sel, instance_stack);
                self.gather_regs(b, instance_stack);
                self.gather_regs(a, instance_stack);
            }

            module::SignalData::InstanceOutput { instance, ref name } => {
                let output = instance.instantiated_module.outputs.borrow()[name];
                self.gather_regs(output, &instance_stack.push(instance));
            }
        }
    }

    pub fn compile_signal(
        &mut self,
        signal: &'a module::Signal<'a>,
        instance_stack: &InstanceStack<'a>,
    ) -> Expr {
        let key = (instance_stack.clone(), signal as *const _);
        if !self.signal_exprs.contains_key(&key) {
            let expr = match signal.data {
                module::SignalData::Lit {
                    ref value,
                    bit_width,
                } => {
                    let value = match value {
                        module::Value::Bool(value) => *value as u128,
                        module::Value::U32(value) => *value as u128,
                        module::Value::U64(value) => *value as u128,
                        module::Value::U128(value) => *value,
                    };

                    let target_type = ValueType::from_bit_width(bit_width);
                    Expr::Value {
                        value: match target_type {
                            ValueType::Bool => Value::Bool(value != 0),
                            ValueType::U32 => Value::U32(value as _),
                            ValueType::U64 => Value::U64(value as _),
                            ValueType::U128 => Value::U128(value),
                        },
                    }
                }

                module::SignalData::Input {
                    ref name,
                    bit_width,
                } => {
                    if let Some((instance, instance_stack_tail)) = instance_stack.pop() {
                        let instance = unsafe { &*instance };
                        self.compile_signal(
                            instance.driven_inputs.borrow()[name],
                            &instance_stack_tail,
                        )
                    } else {
                        let target_type = ValueType::from_bit_width(bit_width);
                        let expr = Expr::Ref {
                            name: name.clone(),
                            scope: RefScope::Member,
                        };
                        self.gen_mask(expr, bit_width, target_type)
                    }
                }

                module::SignalData::Reg { .. } => Expr::Ref {
                    name: self.reg_names[&key].value_name.clone(),
                    scope: RefScope::Member,
                },

                module::SignalData::UnOp { source, op } => {
                    let expr = self.compile_signal(source, instance_stack);
                    let expr = self.gen_temp(Expr::UnOp {
                        source: Box::new(expr),
                        op: match op {
                            module::UnOp::Not => UnOp::Not,
                        },
                    });

                    let bit_width = source.bit_width();
                    let target_type = ValueType::from_bit_width(bit_width);
                    self.gen_mask(expr, bit_width, target_type)
                }
                module::SignalData::BinOp { lhs, rhs, op, .. } => {
                    let source_type = ValueType::from_bit_width(lhs.bit_width());
                    let lhs = self.compile_signal(lhs, instance_stack);
                    let rhs = self.compile_signal(rhs, instance_stack);
                    let op_input_type = match (op, source_type) {
                        (module::BinOp::Add, ValueType::Bool) => ValueType::U32,
                        _ => source_type,
                    };
                    let lhs = self.gen_cast(lhs, source_type, op_input_type);
                    let rhs = self.gen_cast(rhs, source_type, op_input_type);
                    let expr = self.gen_temp(Expr::BinOp {
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                        op: match op {
                            module::BinOp::Add => BinOp::Add,
                            module::BinOp::BitAnd => BinOp::BitAnd,
                            module::BinOp::BitOr => BinOp::BitOr,
                            module::BinOp::BitXor => BinOp::BitXor,
                            module::BinOp::Equal => BinOp::Equal,
                            module::BinOp::NotEqual => BinOp::NotEqual,
                            module::BinOp::LessThan => BinOp::LessThan,
                            module::BinOp::LessThanEqual => BinOp::LessThanEqual,
                            module::BinOp::GreaterThan => BinOp::GreaterThan,
                            module::BinOp::GreaterThanEqual => BinOp::GreaterThanEqual,
                        },
                    });
                    let op_output_type = match op {
                        module::BinOp::Equal
                        | module::BinOp::NotEqual
                        | module::BinOp::LessThan
                        | module::BinOp::LessThanEqual
                        | module::BinOp::GreaterThan
                        | module::BinOp::GreaterThanEqual => ValueType::Bool,
                        _ => op_input_type,
                    };
                    let target_bit_width = signal.bit_width();
                    let target_type = ValueType::from_bit_width(target_bit_width);
                    let expr = self.gen_cast(expr, op_output_type, target_type);
                    self.gen_mask(expr, target_bit_width, target_type)
                }

                module::SignalData::Bits {
                    source, range_low, ..
                } => {
                    let expr = self.compile_signal(source, instance_stack);
                    let expr = self.gen_shift_right(expr, range_low);
                    let target_bit_width = signal.bit_width();
                    let target_type = ValueType::from_bit_width(target_bit_width);
                    let expr = self.gen_cast(
                        expr,
                        ValueType::from_bit_width(source.bit_width()),
                        target_type,
                    );
                    self.gen_mask(expr, target_bit_width, target_type)
                }

                module::SignalData::Repeat { source, count } => {
                    let expr = self.compile_signal(source, instance_stack);
                    let mut expr = self.gen_cast(
                        expr,
                        ValueType::from_bit_width(source.bit_width()),
                        ValueType::from_bit_width(signal.bit_width()),
                    );

                    if count > 1 {
                        let source_expr = expr.clone();

                        for i in 1..count {
                            let rhs =
                                self.gen_shift_left(source_expr.clone(), i * source.bit_width());
                            expr = self.gen_temp(Expr::BinOp {
                                lhs: Box::new(expr),
                                rhs: Box::new(rhs),
                                op: BinOp::BitOr,
                            });
                        }
                    }

                    expr
                }
                module::SignalData::Concat { lhs, rhs } => {
                    let lhs_type = ValueType::from_bit_width(lhs.bit_width());
                    let rhs_bit_width = rhs.bit_width();
                    let rhs_type = ValueType::from_bit_width(rhs_bit_width);
                    let lhs = self.compile_signal(lhs, instance_stack);
                    let rhs = self.compile_signal(rhs, instance_stack);
                    let target_type = ValueType::from_bit_width(signal.bit_width());
                    let lhs = self.gen_cast(lhs, lhs_type, target_type);
                    let rhs = self.gen_cast(rhs, rhs_type, target_type);
                    let lhs = self.gen_shift_left(lhs, rhs_bit_width);
                    self.gen_temp(Expr::BinOp {
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                        op: BinOp::BitOr,
                    })
                }

                module::SignalData::Mux { a, b, sel } => {
                    let lhs = self.compile_signal(b, instance_stack);
                    let rhs = self.compile_signal(a, instance_stack);
                    let cond = self.compile_signal(sel, instance_stack);
                    self.gen_temp(Expr::Ternary {
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                        cond: Box::new(cond),
                    })
                }

                module::SignalData::InstanceOutput { instance, ref name } => {
                    let output = instance.instantiated_module.outputs.borrow()[name];
                    self.compile_signal(output, &instance_stack.push(instance))
                }
            };
            self.signal_exprs.insert(key.clone(), expr);
        }

        self.signal_exprs[&key].clone()
    }

    fn gen_temp(&mut self, expr: Expr) -> Expr {
        let target_name = format!("__temp_{}", self.local_count);
        self.local_count += 1;
        self.prop_assignments.push(Assignment {
            target_scope: TargetScope::Local,
            target_name: target_name.clone(),
            expr,
        });

        Expr::Ref {
            scope: RefScope::Local,
            name: target_name,
        }
    }

    fn gen_mask(&mut self, expr: Expr, bit_width: u32, target_type: ValueType) -> Expr {
        if bit_width == target_type.bit_width() {
            return expr;
        }

        let mask = (1u128 << bit_width) - 1;
        self.gen_temp(Expr::BinOp {
            lhs: Box::new(expr),
            rhs: Box::new(Expr::Value {
                value: match target_type {
                    ValueType::Bool => unreachable!(),
                    ValueType::U32 => Value::U32(mask as _),
                    ValueType::U64 => Value::U64(mask as _),
                    ValueType::U128 => Value::U128(mask),
                },
            }),
            op: BinOp::BitAnd,
        })
    }

    fn gen_shift_left(&mut self, expr: Expr, shift: u32) -> Expr {
        if shift == 0 {
            return expr;
        }

        self.gen_temp(Expr::BinOp {
            lhs: Box::new(expr),
            rhs: Box::new(Expr::Value {
                value: Value::U32(shift),
            }),
            op: BinOp::Shl,
        })
    }

    fn gen_shift_right(&mut self, expr: Expr, shift: u32) -> Expr {
        if shift == 0 {
            return expr;
        }

        self.gen_temp(Expr::BinOp {
            lhs: Box::new(expr),
            rhs: Box::new(Expr::Value {
                value: Value::U32(shift),
            }),
            op: BinOp::Shr,
        })
    }

    fn gen_cast(&mut self, expr: Expr, source_type: ValueType, target_type: ValueType) -> Expr {
        if source_type == target_type {
            return expr;
        }

        if target_type == ValueType::Bool {
            let expr = self.gen_mask(expr, 1, source_type);
            return self.gen_temp(Expr::BinOp {
                lhs: Box::new(expr),
                rhs: Box::new(Expr::Value {
                    value: match source_type {
                        ValueType::Bool => unreachable!(),
                        ValueType::U32 => Value::U32(0),
                        ValueType::U64 => Value::U64(0),
                        ValueType::U128 => Value::U128(0),
                    },
                }),
                op: BinOp::NotEqual,
            });
        }

        self.gen_temp(Expr::Cast {
            source: Box::new(expr),
            target_type,
        })
    }
}

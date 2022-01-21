#[allow(unused_imports)]
use builtin::*;
mod pervasive;
use pervasive::*;

use state_machines_macros::construct_state_machine;

construct_state_machine!(
    state machine X {
        fields {
            #[sharding(variable)]
            counter: int,

            #[sharding(variable)]
            inc_a: bool,

            #[sharding(variable)]
            inc_b: bool,
        }

        #[invariant]
        #[spec]
        fn main_inv(&self) -> bool {
            self.counter == (if self.inc_a { 1 } else { 0 }) + (if self.inc_b { 1 } else { 0 })
        }

        #[init]
        #[spec]
        fn initialize(&self) {
            update(counter, 0);
            update(inc_a, false);
            update(inc_b, false);
        }

        #[transition]
        #[spec]
        fn tr_inc_a(&self) {
            require(!self.inc_a);
            update(counter, self.counter + 1);
            update(inc_a, true);
        }

        #[transition]
        #[spec]
        fn tr_inc_b(&self) {
            require(!self.inc_b);
            update(counter, self.counter + 1);
            update(inc_b, true);
        }

        #[readonly]
        #[spec]
        fn finalize(&self) {
            require(self.inc_a);
            require(self.inc_b);
            assert(self.counter == 2);
        }

        #[proof]
        #[inductive(tr_inc_a)]
        fn tr_inc_a_preserves(pre: X) {
        }

        #[proof]
        #[inductive(tr_inc_b)]
        fn tr_inc_b_preserves(pre: X) {
        }

        #[proof]
        #[inductive(initialize)]
        fn initialize_inv(pre: X) {
        }

        #[proof]
        #[safety(finalize)]
        fn finalize_correct(pre: X) {
        }
    }
);

fn main() { }
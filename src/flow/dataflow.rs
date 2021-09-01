use crate::core::{
  aliases::Aliases,
  control_dependencies::ControlDependencies,
  indexed::{IndexMatrix, IndexSetIteratorExt, IndexedDomain},
  indexed_impls::{arg_location, build_location_domain, LocationDomain, LocationSet, PlaceDomain},
  utils::{self, PlaceCollector, PlaceRelation},
};

use rustc_middle::{
  mir::{visit::Visitor, *},
  ty::TyCtxt,
};
use rustc_mir::dataflow::{Analysis, AnalysisDomain, Forward};
use std::rc::Rc;

pub type FlowDomain<'tcx> = IndexMatrix<Place<'tcx>, Location>;

struct TransferFunction<'a, 'b, 'tcx> {
  analysis: &'a FlowAnalysis<'b, 'tcx>,
  state: &'a mut FlowDomain<'tcx>,
}

impl TransferFunction<'_, '_, 'tcx> {
  fn apply_mutation(
    &mut self,
    mutated: Place<'tcx>,
    inputs: &[Place<'tcx>],
    location: Location,
    definitely_mutated: bool,
  ) {
    let place_domain = self.analysis.place_domain();
    let location_domain = self.analysis.location_domain();

    let mut input_deps: LocationSet = inputs
      .iter()
      .map(|place| self.state.row_indices(*place))
      .flatten()
      .collect_indices(location_domain.clone());

    let controlled_by = self
      .analysis
      .control_dependencies
      .dependent_on(location.block);
    let body = self.analysis.body;
    for block in controlled_by.into_iter().map(|set| set.iter()).flatten() {
      input_deps.insert(body.terminator_loc(block));

      let terminator = body.basic_blocks()[block].terminator();
      if let TerminatorKind::SwitchInt { discr, .. } = &terminator.kind {
        if let Some(discr_place) = utils::operand_to_place(discr) {
          if let Some(discr_deps) = self.state.row_set(discr_place) {
            input_deps.union(&discr_deps);
          }
        }
      }
    }

    input_deps.insert(location);

    let mutated_deps = self.state.row_set(mutated).map(|s| s.to_owned());
    let aliases = self.analysis.aliases.loans(mutated);
    for alias in aliases.iter() {
      for (place_idx, place) in place_domain.iter_enumerated() {
        let relation = PlaceRelation::of(*place, *alias);

        // TODO:
        //   - aliases.len() == 1 is too conservative, should be "if only one pointer at every level of indirection" maybe?
        if definitely_mutated && aliases.len() == 1 && relation == PlaceRelation::Sub {
          self.state.clear_row(place_idx);
        }

        if relation.overlaps() {
          self.state.union_into_row(place_idx, &input_deps);

          // if a value is indirectly mutated, then its dependencies needs to
          // include the provenance of the reference
          if *alias != mutated {
            if let Some(mutated_deps) = mutated_deps.as_ref() {
              self.state.union_into_row(place_idx, mutated_deps);
            }
          }
        }
      }
    }
  }
}

impl Visitor<'tcx> for TransferFunction<'a, 'b, 'tcx> {
  fn visit_assign(&mut self, place: &Place<'tcx>, rvalue: &Rvalue<'tcx>, location: Location) {
    let mut collector = PlaceCollector::default();
    collector.visit_rvalue(rvalue, location);
    self.apply_mutation(*place, &collector.places, location, true);
  }

  fn visit_terminator(&mut self, terminator: &Terminator<'tcx>, location: Location) {
    let tcx = self.analysis.tcx;

    match &terminator.kind {
      TerminatorKind::Call {
        /*func,*/ // TODO: deal with func
        args,
        destination,
        ..
      } => {
        let arg_places = utils::arg_places(args);

        if let Some((dst_place, _)) = destination {
          self.apply_mutation(*dst_place, &arg_places, location, true);
        }

        for mut_ptr in utils::arg_mut_ptrs(&arg_places, tcx, self.analysis.body) {
          self.apply_mutation(mut_ptr, &arg_places, location, false);
        }
      }

      TerminatorKind::DropAndReplace { place, value, .. } => {
        if let Some(src) = utils::operand_to_place(value) {
          self.apply_mutation(*place, &[src], location, true);
        }
      }

      _ => {}
    }
  }
}

pub struct FlowAnalysis<'a, 'tcx> {
  pub tcx: TyCtxt<'tcx>,
  pub body: &'a Body<'tcx>,
  pub control_dependencies: ControlDependencies,
  pub aliases: Aliases<'tcx>,
  pub location_domain: Rc<LocationDomain>,
}

impl FlowAnalysis<'a, 'tcx> {
  pub fn new(
    tcx: TyCtxt<'tcx>,
    body: &'a Body<'tcx>,
    aliases: Aliases<'tcx>,
    control_dependencies: ControlDependencies,
  ) -> Self {
    let location_domain = build_location_domain(body);

    FlowAnalysis {
      tcx,
      body,
      aliases,
      location_domain,
      control_dependencies,
    }
  }

  pub fn place_domain(&self) -> &Rc<PlaceDomain<'tcx>> {
    &self.aliases.place_domain
  }

  pub fn location_domain(&self) -> &Rc<LocationDomain> {
    &self.location_domain
  }
}

impl AnalysisDomain<'tcx> for FlowAnalysis<'a, 'tcx> {
  type Domain = FlowDomain<'tcx>;
  type Direction = Forward;
  const NAME: &'static str = "FlowAnalysis";

  fn bottom_value(&self, _body: &Body<'tcx>) -> Self::Domain {
    FlowDomain::new(self.place_domain().clone(), self.location_domain().clone())
  }

  fn initialize_start_block(&self, body: &Body<'tcx>, state: &mut Self::Domain) {
    for arg in body.args_iter() {
      state.insert(
        utils::local_to_place(arg, self.tcx),
        arg_location(arg, body),
      );
    }
  }
}

impl Analysis<'tcx> for FlowAnalysis<'a, 'tcx> {
  fn apply_statement_effect(
    &self,
    state: &mut Self::Domain,
    statement: &Statement<'tcx>,
    location: Location,
  ) {
    let mut tf = TransferFunction {
      state,
      analysis: self,
    };
    tf.visit_statement(statement, location);
  }

  fn apply_terminator_effect(
    &self,
    state: &mut Self::Domain,
    terminator: &Terminator<'tcx>,
    location: Location,
  ) {
    let mut tf = TransferFunction {
      state,
      analysis: self,
    };
    tf.visit_terminator(terminator, location);
  }

  fn apply_call_return_effect(
    &self,
    _state: &mut Self::Domain,
    _block: BasicBlock,
    _func: &Operand<'tcx>,
    _args: &[Operand<'tcx>],
    _return_place: Place<'tcx>,
  ) {
  }
}

use core::fmt::Debug;
use std::collections::VecDeque;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use ndarray::{Array, Array1, Array2, Axis};
use ndarray_rand::rand_distr::Uniform;
use ndarray_rand::RandomExt;
use plotly::{Bar, Plot};
use rand::{prelude::*, thread_rng, Rng};

use crate::activation::{Activation, Identity, Relu};
use crate::loss::{Loss, MSE};
use crate::substrate::Substrate;

#[derive(Debug)]
pub struct Layer {
    pub x: Array2<f64>,
    pub wi: Array2<usize>,
    pub bi: Array1<usize>,
    pub w: Array2<f64>,
    pub b: Array1<f64>,
    pub d_z: Array2<f64>,
    pub grad_w: Array2<f64>,
    pub grad_b: Array1<f64>,
    pub activation: Rc<dyn Activation>,
}

impl Layer {
    pub fn new(
        pool_size: usize,
        x_shape: (usize, usize),
        w_shape: (usize, usize),
        b_shape: usize,
        activation: Rc<dyn Activation>,
    ) -> Layer {
        Layer {
            x: Array2::zeros(x_shape),
            wi: Array2::random(w_shape, Uniform::new(0, pool_size)),
            bi: Array::random(b_shape, Uniform::new(0, pool_size)),
            w: Array2::zeros(w_shape),
            b: Array::zeros(b_shape),
            d_z: Array2::zeros(w_shape),
            grad_w: Array2::zeros(w_shape),
            grad_b: Array::zeros(b_shape),
            activation,
        }
    }

    pub fn gather(&mut self, substrate: &Substrate) -> &mut Self {
        self.w = self.wi.map(|ix| substrate.get(*ix));
        return self;
    }

    pub fn shift_weights(&mut self, shift: &Array2<usize>) -> &mut Self {
        self.wi += shift;
        return self;
    }

    pub fn shift_bias(&mut self, shift: &Array1<usize>) -> &mut Self {
        self.bi += shift;
        return self;
    }

    pub fn assign_grad_w(&mut self, grad: Array2<f64>) -> &mut Self {
        self.grad_w = grad;
        self
    }

    pub fn assign_grad_b(&mut self, grad: Array1<f64>) -> &mut Self {
        self.grad_b = grad;
        self
    }

    pub fn forward(&mut self, x: Array2<f64>) -> Array2<f64> {
        self.x = x.clone();
        let z = x.dot(&self.w) + &self.b;
        let a_z = self.activation.a(z.clone());
        let d_z = self.activation.d(z.clone());
        self.d_z = d_z;
        a_z
    }

    pub fn backward(&mut self, grad_output: Array2<f64>) -> Array2<f64> {
        let grad_z = grad_output * &self.d_z;
        let grad_input = grad_z.dot(&self.w.t());
        let grad_w = self.x.t().dot(&grad_z);
        let grad_b = grad_z.sum_axis(Axis(0));

        self.grad_w -= &(grad_w);
        self.grad_b -= &(grad_b);

        grad_input
    }
}

pub enum GradientRetention {
    Roll,
    Zero,
}

pub type LayerSchema = Vec<usize>;
pub type Web = Vec<Layer>;

pub struct Manifold {
    substrate: Arc<Substrate>,
    d_in: usize,
    d_out: usize,
    layers: LayerSchema,
    web: Web,
    hidden_activation: Rc<dyn Activation>,
    output_activation: Rc<dyn Activation>,
    verbose: bool,
    loss: Rc<dyn Loss>,
    gradient_retention: GradientRetention,
    learning_rate: f64,
    decay: f64,
    early_terminate: Box<dyn Fn(&Vec<f64>) -> bool>,
    epochs: usize,
    sample_size: usize,
    losses: Vec<f64>,
}

impl Manifold {
    pub fn new(
        substrate: Arc<Substrate>,
        d_in: usize,
        d_out: usize,
        layers: Vec<usize>,
    ) -> Manifold {
        Manifold {
            substrate,
            d_in,
            d_out,
            layers,
            web: Web::new(),
            hidden_activation: Relu::new(),
            output_activation: Identity::new(),
            verbose: false,
            loss: MSE::new(),
            gradient_retention: GradientRetention::Roll,
            learning_rate: 0.001,
            decay: 1.,
            early_terminate: Box::new(|_| false),
            epochs: 1000,
            sample_size: 10,
            losses: vec![],
        }
    }

    pub fn dynamic(
        substrate: Arc<Substrate>,
        d_in: usize,
        d_out: usize,
        breadth: Range<usize>,
        depth: Range<usize>,
    ) -> Manifold {
        let mut rng = thread_rng();
        let depth = rng.gen_range(depth);
        let layers = (0..depth)
            .map(|_| rng.gen_range(breadth.clone()))
            .collect::<Vec<usize>>();

        Manifold {
            substrate,
            d_in,
            d_out,
            web: Web::new(),
            layers,
            hidden_activation: Relu::new(),
            output_activation: Identity::new(),
            verbose: false,
            loss: MSE::new(),
            gradient_retention: GradientRetention::Roll,
            learning_rate: 0.001,
            decay: 1.,
            early_terminate: Box::new(|_| false),
            epochs: 1000,
            sample_size: 1,
            losses: vec![],
        }
    }

    pub fn set_hidden_activation(&mut self, activation: Rc<dyn Activation>) -> &mut Self {
        self.hidden_activation = activation;
        self
    }

    pub fn set_output_activation(&mut self, activation: Rc<dyn Activation>) -> &mut Self {
        self.output_activation = activation;
        self
    }

    pub fn verbose(&mut self) -> &mut Self {
        self.verbose = true;
        self
    }

    pub fn set_loss(&mut self, loss: Rc<dyn Loss>) -> &mut Self {
        self.loss = loss;
        self
    }

    pub fn set_gradient_retention(&mut self, method: GradientRetention) -> &mut Self {
        self.gradient_retention = method;
        self
    }

    pub fn set_learning_rate(&mut self, rate: f64) -> &mut Self {
        self.learning_rate = rate;
        self
    }

    pub fn set_decay(&mut self, decay: f64) -> &mut Self {
        self.decay = decay;
        self
    }

    pub fn until(&mut self, patience: usize, min_delta: f64) -> &mut Self {
        let early_terminate = move |losses: &Vec<f64>| {
            let mut deltas: Vec<f64> = vec![];
            let len = losses.len();

            if patience + 2 > len {
                return false;
            }

            for i in ((len - patience)..len).rev() {
                let c = losses[i];
                let c2 = losses[i - 1];

                let delta = c2 - c;
                deltas.push(delta);
            }

            let avg_delta = deltas.iter().fold(0., |a, v| a + *v) / deltas.len() as f64;

            println!("avg delta {}", avg_delta);

            if avg_delta < min_delta {
                return true;
            }

            return false;
        };

        self.early_terminate = Box::new(early_terminate);
        self
    }

    pub fn until_some(
        &mut self,
        early_terminate: impl Fn(&Vec<f64>) -> bool + 'static,
    ) -> &mut Self {
        self.early_terminate = Box::new(early_terminate);
        self
    }

    pub fn set_epochs(&mut self, epochs: usize) -> &mut Self {
        self.epochs = epochs;
        self
    }

    pub fn set_sample_size(&mut self, sample_size: usize) -> &mut Self {
        self.sample_size = sample_size;
        self
    }

    pub fn weave(&mut self) -> &mut Self {
        let mut x_shape = (1, self.d_in);
        let mut w_shape: (usize, usize);
        let mut b_shape: usize;
        let mut p_dim = self.d_in;

        for layer_size in self.layers.iter() {
            w_shape = (p_dim, *layer_size);
            b_shape = w_shape.1;

            self.web.push(Layer::new(
                self.substrate.size,
                x_shape,
                w_shape,
                b_shape,
                Rc::clone(&self.hidden_activation),
            ));
            p_dim = *layer_size;
            x_shape = (1, w_shape.1);
        }

        let w_shape = (p_dim, self.d_out);
        let b_shape = w_shape.1;

        self.web.push(Layer::new(
            self.substrate.size,
            x_shape,
            w_shape,
            b_shape,
            Rc::clone(&self.output_activation),
        ));
        self
    }

    pub fn gather(&mut self) -> &mut Self {
        for layer in self.web.iter_mut() {
            layer.gather(&self.substrate);
        }
        self
    }

    fn prepare(&self, x: Vec<f64>) -> Array2<f64> {
        let l = x.len();
        let mut xvd: VecDeque<f64> = VecDeque::from(x);
        Array2::zeros((1, l)).mapv_into(|_| xvd.pop_front().unwrap())
    }

    pub fn forward(&mut self, xv: Vec<f64>) -> Array1<f64> {
        let mut x = self.prepare(xv);
        for layer in self.web.iter_mut() {
            x = layer.forward(x);
        }
        let shape = x.len();
        x.into_shape(shape).unwrap()
    }

    pub fn backwards(&mut self, y_pred: Array1<f64>, y: Vec<f64>, loss: Rc<dyn Loss>) {
        let y_target = Array1::from(y);
        let grad_output_i = loss.d(y_pred, y_target);

        let grad_output_shape = (1, grad_output_i.len());
        let mut grad_output = grad_output_i.into_shape(grad_output_shape).unwrap();

        for layer in self.web.iter_mut().rev() {
            grad_output = layer.backward(grad_output);

            let grad_b_dim = layer.grad_b.raw_dim();
            let grad_w_dim = layer.grad_w.raw_dim();

            let mut b_grad_reshaped = layer.grad_b.to_owned().insert_axis(Axis(1));
            let mut b_link_reshaped = layer.bi.to_owned().insert_axis(Axis(1));

            self.substrate
                .highspeed(&mut layer.grad_w, &mut layer.wi, self.learning_rate);
            self.substrate.highspeed(
                &mut b_grad_reshaped,
                &mut b_link_reshaped,
                self.learning_rate,
            );

            layer
                .shift_bias(&b_link_reshaped.remove_axis(Axis(1)))
                .assign_grad_b(b_grad_reshaped.remove_axis(Axis(1)))
                .gather(&self.substrate);

            match self.gradient_retention {
                GradientRetention::Zero => {
                    layer
                        .assign_grad_b(Array1::zeros(grad_b_dim))
                        .assign_grad_w(Array2::zeros(grad_w_dim));
                }
                GradientRetention::Roll => (),
            }
        }
    }

    pub fn train(&mut self, x: Vec<Vec<f64>>, y: Vec<Vec<f64>>) -> &mut Self {
        let xy = x
            .into_iter()
            .zip(y.into_iter())
            .collect::<Vec<(Vec<f64>, Vec<f64>)>>();
        let mut rng = thread_rng();

        for epoch in 0..self.epochs {
            let sample = xy
                .choose_multiple(&mut rng, self.sample_size)
                .collect::<Vec<&(Vec<f64>, Vec<f64>)>>();
            let mut total_loss: Vec<f64> = vec![];

            for &xy in sample.iter() {
                let (x, y) = xy.clone();

                let y_pred = self.forward(x);
                total_loss.push(self.loss.a(y_pred.clone(), Array1::from(y.clone())));
                self.backwards(y_pred, y, Rc::clone(&self.loss));
            }

            self.learning_rate *= self.decay;

            let ct = total_loss.len() as f64;
            let avg_loss = total_loss.into_iter().fold(0., |a, v| a + v) / ct;
            self.losses.push(avg_loss);

            if (self.early_terminate)(&self.losses) {
                if self.verbose {
                    println!("Early termination condition met.");
                }

                break;
            }

            if self.verbose {
                println!("({}/{}) Loss = {}", epoch, self.epochs, avg_loss);
            }
        }

        self
    }

    pub fn loss_graph(&mut self) -> &mut Self {
        let mut plot = Plot::new();

        let x = (0..self.losses.len()).collect();

        let trace = Bar::new(x, self.losses.clone());
        plot.add_trace(trace);
        plot.write_html("loss.html");
        plot.show();

        self
    }
}

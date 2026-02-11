import { Routes, Route } from 'react-router-dom';
import Layout from './components/Layout';
import Chat from './pages/Chat';
import Agent from './pages/Agent';
import Graph from './pages/Graph';
import Research from './pages/Research';
import Pipeline from './pages/Pipeline';
import Traces from './pages/Traces';
import TimeTravel from './pages/TimeTravel';
import Projects from './pages/Projects';
import Dashboard from './pages/Dashboard';

export default function App() {
  return (
    <Routes>
      <Route element={<Layout />}>
        <Route index element={<Chat />} />
        <Route path="agent" element={<Agent />} />
        <Route path="graph" element={<Graph />} />
        <Route path="research" element={<Research />} />
        <Route path="pipeline" element={<Pipeline />} />
        <Route path="traces" element={<Traces />} />
        <Route path="time-travel" element={<TimeTravel />} />
        <Route path="projects" element={<Projects />} />
        <Route path="dashboard" element={<Dashboard />} />
      </Route>
    </Routes>
  );
}
